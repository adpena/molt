# Bleeding-Edge GPU Inference Optimization Research

**Date:** 2026-04-14
**Scope:** Techniques for maximum performance in molt's tinygrad GPU primitive stack

## Highest-Impact Techniques (Implement Now)

### 1. Shape Specialization / Monomorphization
- **What:** Generate optimal kernels for known shapes at AOT compile time
- **Impact:** Smaller binary, faster startup, no runtime shape checks
- **Difficulty:** Trivial — already in molt's design
- **Action:** Implement in `schedule.rs` — specialize FusedKernel grid/local for known shapes

### 2. Constant Folding Through Lazy DAG
- **What:** Evaluate static computations at compile time, embed as FusedSrc::Const
- **Impact:** Reduces runtime ops, improves cache locality
- **Difficulty:** Trivial — standard dataflow analysis
- **Action:** Add constant folding pass in `fuse.rs`

### 3. Prefix Caching
- **What:** Cache KV blocks for prompt prefixes, 85-90% latency reduction for long prompts
- **Impact:** 85-90% latency reduction, 50-90% cost reduction
- **Difficulty:** Moderate
- **Action:** Extend `kv_cache.py` with prefix tree (radix tree) for shared prefixes

### 4. MXFP (Microscaling Floating Point) Support
- **What:** Industry-standard block floating point (OCP spec v1.0), supported by AMD/ARM/Intel/Meta/NVIDIA/Qualcomm
- **Impact:** MXFP8 <1% accuracy drop, portable across all hardware
- **Difficulty:** Moderate — standard format
- **Action:** Add MXFP8/MXFP4 to DType enum, implement in renderers

### 5. Lazy Kernel Compilation + Caching
- **What:** Compile on first use, cache per-device compiled kernels
- **Impact:** 85% startup time reduction
- **Difficulty:** Moderate
- **Action:** Already in MetalDevice Compiler impl; extend to all backends

### 6. Dead Code Elimination at Kernel Level
- **What:** Remove unused ops from lazy DAG before fusion
- **Impact:** Smaller models, faster execution
- **Difficulty:** Trivial — standard DCE
- **Action:** Add DCE pass before `schedule()`

## High-Impact Techniques (Implement Q2-Q3 2026)

### 7. FlashAttention-3 Kernel Patterns
- **Impact:** 1.5-2x on Hopper/Blackwell, 75% utilization
- **Difficulty:** Hard — register-level GPU programming
- **Action:** Implement tiled attention in MslRenderer/CudaRenderer

### 8. NVFP4 Quantization (Blackwell)
- **Impact:** 4-6x inference speedup, 3.5x memory reduction
- **Difficulty:** Moderate — Blackwell-specific
- **Action:** Add NVFP4 to DType, CudaRenderer FP4 Tensor Core ops

### 9. Target-Conditioned Speculative Decoding
- **Impact:** 3-6.5x speedup, lossless
- **Difficulty:** Hard — prediction head training
- **Action:** Implement trained adapters under `src/molt/gpu/dflash/`. Do not
  extend generic helper modules and call them DFlash.

### 10. ThunderKittens Integration
- **Impact:** 2-2.6x for custom fused ops, <50 lines per new op
- **Difficulty:** Moderate — tile abstraction learning curve
- **Action:** Use TK for attention/MLA kernels on CUDA backend

## Research-Level (Q4 2026+)

### 11. Persistent Kernels / Mega-Kernels
- **Impact:** 13.8% latency reduction, no kernel launch overhead
- **Difficulty:** Research — novel IR required

### 12. Ring Attention
- **Impact:** device_count x context length
- **Difficulty:** Hard — ring communication DAG

### 13. Warp-Specialization Automation (Tawa)
- **Impact:** 1.1-1.2x over cuBLAS GEMM
- **Difficulty:** Hard — compiler IR + schedule exploration

## Key Papers

| Paper | Year | Key Result |
|-------|------|-----------|
| FlashAttention-3 | 2024 | 740 TFLOPs/s on H100 |
| ThunderKittens 2.0 | 2026 | 2.6x over NCCL multi-GPU |
| EAGLE-3 | 2025 | 3-6.5x speculative decoding |
| TurboQuant | 2026 | 6x KV cache, 0% accuracy loss |
| Mirage Mega-Kernel | 2025 | 13.8% latency reduction |
| TritonForge | 2025 | 5x over baseline Triton |
| Tawa | 2025 | 1.1x over cuBLAS GEMM |
| FlashInfer | 2025 | MLSys Best Paper, SM75-Blackwell |
| MXFP Spec | 2024 | Industry standard, all vendors |
| NVFP4 | 2026 | 4-6x on Blackwell native |
