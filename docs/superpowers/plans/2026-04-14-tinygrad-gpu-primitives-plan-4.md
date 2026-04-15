# Tinygrad GPU Primitives — Plan 4: TurboQuant + DFlash + DDTree

**Goal:** Implement the three advanced ML primitives as pure compositions of the 26-op stack: TurboQuant (quantization), DFlash (flash attention with speculative decoding), and DDTree (dynamic decision tree for routing).

**Depends on:** Plan 3 (complete)

---

## File Map

| Path | Responsibility |
| --- | --- |
| `stdlib/tinygrad/turbo_quant.py` | TurboQuant — int4/int8 quantization via primitives |
| `stdlib/tinygrad/dflash.py` | DFlash — flash attention + speculative decoding |
| `stdlib/tinygrad/ddtree.py` | DDTree — dynamic decision tree for MoE routing |
| `tests/gpu/test_turbo_quant.py` | TurboQuant accuracy and performance tests |
| `tests/gpu/test_dflash.py` | DFlash correctness and fusion count tests |
| `tests/gpu/test_ddtree.py` | DDTree routing accuracy tests |

## Tasks

### Task 1: TurboQuant — Int4/Int8 Quantization
- Symmetric quantization: `q = CAST(ROUND(x / scale), int8)` where `scale = MAX(ABS(x)) / 127`
- Asymmetric quantization: with zero-point adjustment
- Block quantization: per-block scale factors (block_size=32 or 128)
- Dequantization: `x_hat = CAST(q, float32) * scale`
- All expressed as primitive op compositions — no new primitives
- Correctness: max quantization error within 1 ULP of reference

### Task 2: TurboQuant — Mixed Precision Matmul
- `matmul_q4(A_f16, B_int4, scales)` — dequantize B on-the-fly during matmul
- Fuses: dequant + reshape + expand + mul + reduce_sum into minimal kernels
- Target: 2 kernels for a quantized matmul (dequant+mul fused, then reduce)

### Task 3: DFlash — Flash Attention v2
- Tiled attention: process Q/K/V in blocks to stay in SRAM
- Online softmax: running max + running sum normalization
- Composed from: MATMUL(Q, K^T) -> MUL(_, 1/sqrt(d_k)) -> SOFTMAX -> MATMUL(_, V)
- Causal mask: via PAD with -inf in upper triangle
- Expected fusion: 3-4 kernels total (not 10+ unfused ops)

### Task 4: DFlash — Speculative Decoding
- Draft model generates k candidate tokens
- Verification: parallel evaluation of all k candidates in one batched forward pass
- Acceptance: compare draft probs vs target probs using WHERE/CMPLT
- Rejection sampling: composed from RAND + CMPLT + WHERE
- All expressed as Tensor method compositions

### Task 5: DDTree — Dynamic Decision Tree
- Binary decision tree for MoE expert routing
- Node evaluation: `WHERE(CMPLT(feature[split_dim], threshold), left_child, right_child)`
- Leaf mapping: expert index lookup via gather
- Tree traversal: unrolled as a chain of WHERE ops (no control flow in kernel)
- Expected fusion: entire tree traversal in 1 kernel (all elementwise)

### Task 6: DDTree — Top-K Expert Selection
- After tree routing, select top-k experts per token
- `topk(scores, k)` via iterative argmax + mask
- Load balancing loss: composed from REDUCE_SUM + MUL

### Task 7: Integration Tests
- TurboQuant: quantize -> dequantize round-trip error < 0.5%
- DFlash: attention output matches naive implementation within 1e-3
- DDTree: routing accuracy matches reference decision tree
- All tests run on CPU + Metal backends

### Task 8: Performance Benchmarks
- TurboQuant matmul: target 2x speedup over f32 matmul (memory-bound)
- DFlash: target 3x speedup over naive attention (SRAM-bound)
- DDTree: target <0.1ms for 1024-token batch with 8-expert MoE
- Fusion count verification for each operation

---

## What Plan 4 Delivers

1. TurboQuant int4/int8 quantization as primitive compositions
2. Mixed-precision quantized matmul with kernel fusion
3. Flash attention v2 with tiled computation and online softmax
4. Speculative decoding pipeline
5. Dynamic decision tree for MoE expert routing
6. Performance benchmarks and correctness tests
