# WebGPU Dispatch Overhead for LLM Inference

**Paper**: "Characterizing WebGPU Dispatch Overhead for LLM Inference Across Four GPU Vendors, Three Backends, and Three Browsers"
**Author**: Jedrzej Maczan
**ArXiv**: 2604.02344 (April 2026)
**Code**: https://github.com/jmaczan/torch-webgpu
**Relevance**: Direct -- our molt-gpu WebGPU backend (Falcon-OCR browser inference via Cloudflare Workers) faces the exact dispatch overhead problem this paper characterizes.

---

## 1. Paper Summary

### Core Finding: Naive Benchmarks Overestimate Dispatch Cost by 20x

Single-operation benchmarks measure GPU->CPU synchronization overhead (~450 us), inflating results 10-60x. The sequential-dispatch methodology (100 consecutive dispatches, sync only at the end) reveals the true per-dispatch API overhead:

| Implementation | Single-Op (us) | Sequential (us) | Backend | GPU Vendor |
|---|---|---|---|---|
| Dawn (RTX 5090) | 496.8 | **23.8** | Vulkan | NVIDIA |
| wgpu (RTX 5090) | 35.8 | **35.8** | Vulkan | NVIDIA |
| wgpu (AMD iGPU) | 24.8 | **24.5** | Vulkan | AMD |
| wgpu (Apple M2) | 48.3 | **71.1** | Metal | Apple |
| Chrome (RTX 5090, Linux) | 2071.2 | **32.8** | Vulkan | NVIDIA |
| Chrome (RTX 2000, Windows) | 2728.8 | **58.7** | D3D12 | NVIDIA |
| Chrome (Intel iGPU, Win) | 3123.6 | **66.5** | D3D12 | Intel |
| Safari (Apple M2) | 248.0 | **31.7** | Metal | Apple |
| Firefox (all) | ~104,000 | **~1037** | Multiple | Multiple |

**Critical distinction**: Per-dispatch cost (WebGPU API alone) is 24-36 us on Vulkan and 32-71 us on Metal. Per-operation overhead including Python/framework is ~95 us. Our Rust backend eliminates the Python cost entirely.

### Overhead Decomposition (RTX 5090/Dawn, torch-webgpu)

Total per-operation overhead: ~95 us, broken down (~30% uncertainty):
- WebGPU dispatch: 13-20 ms over 564 ops (24-36 us/op)
- Python/framework overhead: 28-40 ms (interpreter, tensor metadata)
- GPU/CPU pipelining overlap: ~12 ms
- Submission dominates per-dispatch cost at 40%

### Kernel Fusion Impact

| Configuration | Dispatches Saved | Tok/s | TTFT (ms) | Improvement |
|---|---|---|---|---|
| No fusion (baseline) | -- | 13.5 | 71.4 | -- |
| + Fused RMSNorm (6->1) | 240/fwd | 19.4 | 46.6 | +44% |
| + Fused MLP gate+up+silu (3->1) | +48/fwd | 20.5 | 43.3 | +6% |
| + Fused K+V projection (2->1) | +24/fwd | 20.6 | 41.6 | +0.5% |
| **Total** | **312 fewer** | **21.0** | **41.6** | **+53%** |

Fusion patterns implemented:
1. **RMSNorm (6->1)**: pow, mean, add(eps), rsqrt, mul(x), mul(weight) -> single kernel. 24 layers x 2 norms = 240 dispatches saved.
2. **MLP gate+up+SiLU (3->1)**: silu(xW_gate^T) * (xW_up^T) -> single kernel. 24 layers = 48 saved.
3. **K+V projection (2->1)**: same-dim projections in GQA merged. 24 saved (not statistically significant).

**Backend-dependent fusion**: RMSNorm fusion yields 1.4-1.7x on Vulkan but **regresses** on Metal (0.95x wgpu-native, 0.91x Safari). Tiled MLP (7->3 dispatches) helps both: 1.17x Vulkan, 2.0x Metal.

### End-to-End Performance

| Platform | Backend | Dtype | Tok/s |
|---|---|---|---|
| Linux RTX 5090 | CUDA (compiled) | fp16 | 185.5 |
| Linux RTX 5090 | torch-webgpu (fused) | fp32 | 21.0 |
| Linux RTX 5090 | ONNX Runtime WebGPU | fp32 | 13.1 |
| Linux RTX 5090 | CPU (AMD Ryzen) | fp32 | 13.7 |
| Windows RTX PRO 2000 | CUDA | fp32 | 30.1 |
| macOS M2 | MPS | fp16 | 47.8 |
| macOS M2 | MPS | fp32 | 12.9 |
| Chrome Windows | WebLLM q4f16 | q4f16 | 51.1 |
| Chrome macOS | WebLLM q4f16 | q4f16 | 46.4 |
| Safari macOS | WebLLM q4f16 | q4f16 | 41.7 |
| Firefox (all) | WebLLM q4f16 | q4f16 | 9.1-9.6 |

torch-webgpu achieves 11-12% of CUDA fp16 performance. At dtype-matched fp32, RTX PRO 2000 achieves only 1.4x WebGPU's throughput despite ~6x less compute.

### Dispatch-Bound Crossover Analysis

At batch=1, all operations are overhead-bound. Crossover to compute-bound:

| Operation | Dimensions | Crossover Batch (B*) |
|---|---|---|
| Attention Q/K/V (0.5B) | 896x896 | 119 |
| MLP up (0.5B) | 896x4864 | 22 |
| MLP down (0.5B) | 4864x896 | 22 |
| MLP up (1.5B) | 1536x8960 | 7 |

### What DOES NOT Help

| Optimization | Result | Why |
|---|---|---|
| Command batching | Minimal | Autoregressive sync per token flushes batched commands |
| Buffer pooling | Minimal | No measurable benefit |
| Bind group caching | Minimal | No measurable benefit |
| Mega-kernels (full block) | Inconclusive (p>0.38) | Too large for registers |
| Device-side argmax | Inconclusive | Metal shows regression |

### torch-webgpu Architecture

PrivateUse1-based PyTorch out-of-tree backend:
1. FX Graph Lowering: PyTorch FX IR captures computation graphs
2. WGSL Shader Codegen: ops compile to WGSL compute shaders
3. WebGPU Runtime: Google Dawn as implementation

FX graph for Qwen 0.5B: 1,911 nodes, 876 compute dispatches. Unoptimized WGSL matmul achieves 1-2% FP32 peak; third-party evidence suggests ~17% achievable via 2D tiling, loop unrolling, vectorized FMA.

### Scaling Consistency

Per-operation overhead stable across model sizes (~95 us for 0.5B, ~99 us for 1.5B). Fusion benefits increase with depth: 1.56x at 0.5B, 1.72x at 1.5B.

---

## 2. Implications for molt-gpu WebGPU Backend

### Our Advantages Over torch-webgpu

1. **No Python overhead**: We eliminate the ~60 us Python/framework cost per operation. Our per-dispatch cost is the raw WebGPU API overhead: 24-36 us on Vulkan, 32-71 us on Metal.

2. **Pre-existing fusion engine**: Our `fuse.rs` already fuses elementwise->reduce->elementwise chains. The paper confirms this is the highest-impact optimization (53% throughput improvement from fusion alone).

3. **Compiled pipeline cache**: Our `WebGpuDevice` already caches compiled pipelines by source hash. The paper confirms bind group caching and buffer pooling provide no benefit, so we are not missing easy wins there.

4. **Shape specialization**: Our `schedule.rs` already runs shape specialization with bounds check elimination. This is exactly the kind of optimization the paper says matters when dispatch overhead dominates.

### Our Gaps

1. **No multi-dispatch command buffers**: Our `exec()` creates a new command encoder per dispatch. The paper says command batching has minimal impact for autoregressive inference (sync per token), but for Falcon-OCR's non-autoregressive workloads (image processing, embedding computation), batching multiple dispatches into one `queue.submit()` could reduce submission overhead (40% of per-dispatch cost).

2. **No backend-adaptive fusion**: The paper shows RMSNorm fusion **regresses** on Metal. We need to detect the backend and adjust fusion strategy. Tiled MLP fusion (7->3 dispatches) is universally beneficial.

3. **No workgroup size tuning per backend**: Our `PREFERRED_LOCAL_SIZES` is one-size-fits-all. The paper shows backend choice dominates performance, and within Metal alone there is 2.2x variance. Workgroup size may need backend-specific tuning.

4. **No subgroup-aware fusion**: When subgroups are available (Chrome 134+), reduction fusions should prefer subgroup operations over sequential loops. We have subgroup support in `WgslRenderer` but it is not integrated into the fusion decision.

### Performance Ceiling Estimate

For Falcon-OCR browser inference (non-autoregressive, batch=1):
- Assume ~200 dispatches per inference pass (after fusion)
- At 30 us/dispatch (Chrome Vulkan): 6 ms dispatch overhead
- At 65 us/dispatch (Chrome D3D12/Intel): 13 ms dispatch overhead
- At 1000 us/dispatch (Firefox): 200 ms -- Firefox is not viable for real-time

Target: <20 ms total inference latency for OCR. Dispatch overhead alone consumes 30-65% of budget on viable browsers. Every fused dispatch saved is 30-65 us recovered.

---

## 3. Concrete Optimizations to Implement

### P0: Multi-Dispatch Command Buffer Submission

**Paper evidence**: Submission is 40% of per-dispatch cost. For non-autoregressive workloads (Falcon-OCR), we can batch all dispatches for a forward pass into a single command encoder and submit once.

**Implementation**: Add `exec_batch()` to `Executor` trait. `WebGpuDevice::exec_batch()` creates one encoder, encodes all dispatches as separate compute passes, and submits once.

**Expected impact**: ~40% reduction in per-dispatch overhead for batched workloads. At 200 dispatches and 30 us/dispatch, saves ~2.4 ms.

### P0: Backend-Adaptive Fusion Strategy

**Paper evidence**: RMSNorm fusion (6->1) gives 1.4-1.7x on Vulkan but 0.91-0.95x on Metal. Tiled MLP (7->3) gives 1.17x on Vulkan and 2.0x on Metal.

**Implementation**: Query the wgpu adapter backend type at device creation. Store it in `WebGpuDevice`. Pass backend hint to the fusion engine so it can skip Metal-regressing fusions and prefer universally-beneficial patterns.

### P1: Workgroup Size Tuning Per Backend

**Paper evidence**: Backend choice is the dominant performance factor. Within Metal, implementation choice matters 2.2x.

**Implementation**: After querying adapter backend, adjust `PREFERRED_LOCAL_SIZES` or the specialization heuristic. For Metal, prefer smaller workgroup sizes (64-128) over 256. For Vulkan, 256 remains optimal.

### P1: Bounds-Check Elimination in WGSL Renderer

**Paper evidence**: Every instruction in the dispatch path matters when overhead dominates. Bounds checks are pure overhead when shape specialization proves they are unnecessary.

**Implementation**: Already tracked via `ShapeSpecialization::bounds_check_elim`. Wire it into `WgslRenderer::render()` to omit the `if (gid >= N) { return; }` guard when proven safe.

### P2: Pipeline Cache Warming

**Paper evidence**: Pipeline compilation is a one-time cost per unique shader, but it is expensive (hundreds of ms for complex shaders). Compiling lazily on first dispatch adds latency to the first inference.

**Implementation**: Pre-compile all pipelines for the Falcon-OCR model at Worker startup, before the first inference request. Store pre-compiled pipelines in the cache.

---

## 4. Updated Performance Targets for Browser Inference

### Before This Research

| Metric | Target |
|---|---|
| Falcon-OCR latency (browser) | <50 ms |
| Dispatch overhead budget | Unknown |

### After This Research

| Metric | Chrome/Vulkan | Chrome/D3D12 | Safari/Metal | Firefox |
|---|---|---|---|---|
| Per-dispatch overhead | ~30 us | ~60 us | ~32 us | ~1000 us (not viable) |
| 200 dispatches overhead | 6 ms | 12 ms | 6.4 ms | 200 ms |
| With batch submit (-40%) | 3.6 ms | 7.2 ms | 3.8 ms | 120 ms |
| Remaining compute budget | 16.4 ms | 12.8 ms | 16.2 ms | N/A |
| Target total latency | <20 ms | <20 ms | <20 ms | Degraded mode |

### Revised Targets

1. **Chrome (Vulkan/D3D12)**: <20 ms latency achievable with current dispatch count after fusion. Focus on kernel compute efficiency.
2. **Safari (Metal)**: <20 ms achievable. Avoid Metal-regressing fusion patterns. Use tiled MLP fusion instead of full RMSNorm fusion.
3. **Firefox**: Not viable for real-time inference. Degrade gracefully to batch mode or server-side fallback.
4. **Dispatch budget**: Maximum 300 dispatches per inference pass (at 30 us = 9 ms overhead). Target <200 after fusion.
5. **Fusion target**: Reduce raw dispatch count by at least 50% from unfused baseline, matching the paper's 876->564 ratio.

### Key Principle from the Paper

> "At batch=1 with the current dispatch-heavy pipeline, per-operation overhead dominates regardless of kernel quality."

This means for Falcon-OCR at batch=1:
- Optimizing WGSL shader quality has diminishing returns until dispatch count is minimized.
- Every dispatch eliminated is worth 30-65 us.
- Fusion is the single highest-impact optimization.
- Backend detection and adaptive strategy is required for cross-browser correctness.
