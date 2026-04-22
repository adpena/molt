# Tiered KV Caching for LLM Inference

**Date**: 2026-04-14
**Author**: Research synthesis for molt tinygrad GPU primitive stack

---

## 1. Kareto: Adaptive Multi-Objective Tiered Storage for KV Cache (arXiv 2603.08739)

### Paper Summary

**Title**: Adaptive Multi-Objective Tiered Storage Configuration for KV Cache in LLM Service
**Authors**: Xianzhe Zheng, Zhengheng Wang, Ruiyan Ma, Rui Wang et al. (20 researchers)
**Published**: 2026-02-25

The paper addresses the fundamental tension in LLM inference: KV caching accelerates
generation by trading memory for computation, but GPU HBM capacity is limited and
expensive. Offloading KV cache to cheaper storage tiers (DRAM, SSD) expands capacity
but introduces the challenge of dynamically managing heterogeneous storage to balance
cost, throughput, and latency under varying workloads.

### Core Contribution: Kareto Framework

Kareto (KV-cache Adaptive REsource managemenT Optimizer) formulates tiered KV cache
management as a multi-objective optimization problem and finds the Pareto frontier
across three metrics:

**Optimization objective (Equation 1)**:
```
min_{x in X} (E[L(x)], -E[T(x)], C(x))
```
where:
- L(x) = expected latency (TTFT)
- T(x) = expected throughput (tokens/sec)
- C(x) = total cost
- x = [x1, x2, x3, x4] = workload patterns, compute config, storage medium, management policy

**Cost decomposition (Equation 2)**:
```
C(x) = c_hw * GPU-Hours(x) + sum_{k in K} phi_k(s_k(x))
```

### Algorithm 1: Adaptive Pareto Search (Diminishing-Return-Guided Pruning)

1. Start with coarse grid exploration (DRAM: 0-2048 GB step 512; TTL: 0-2400s step 600)
2. Monitor marginal performance gains against expansion threshold tau_e
3. Terminate exploration when gains fall below threshold
4. Refine high-curvature regions where performance varies significantly
5. Achieves comparable Pareto frontier quality to full grid search (200 points) with only 60-80 evaluations

### Algorithm 2: ROI-Aware Group TTL

Partitions requests into K+1 groups based on top-K frequent prefix subtrees:

**Group TTL optimization (Equation 3)**:
```
max_{t in R+^{K+1}} sum_{g=1}^{K+1} H_g(t_g)
s.t. sum_{g=1}^{K+1} C_g(t_g) <= B
```

Where:
- H_g(t) = |{delta in Delta_g : delta <= t}| (cumulative hits)
- C_g(t) = |B_g| * t + sum_{delta in Delta_g} min(t, delta) (storage cost)

Solved via SLSQP with multi-start initialization (floor(sqrt(K)) + 1 perturbed points).

### Three-Tier Storage Model

| Tier | Storage | Bandwidth | Cost | Use Case |
|------|---------|-----------|------|----------|
| Hot  | GPU HBM | Highest   | Highest | Active cache blocks, current generation |
| Warm | Host DRAM | Moderate | Medium | Completed but likely reused (multi-turn) |
| Cold | NVMe SSD | Lowest   | Lowest | Historical context, prefix cache |

### Key Experimental Results

Compared to fixed 1024 GB DRAM baseline:
- **Throughput**: up to 9.3% improvement (compute-constrained 1-instance settings)
- **Latency (TTFT)**: up to 58.3% reduction
- **Cost**: up to 20.2% reduction

Group TTL impact:
- Trace B (API workloads): 15-20% improvement in reuse ratio
- Trace C (agent workloads): 10-15% improvement in reuse ratio
- Trace A (chat): marginal gains (scattered temporal patterns)

Critical observations:
- 31.95% of blocks account for 90% of hits (Trace A)
- 0.67% of blocks account for 90% of hits (Trace B)
- Disk offloading is only beneficial when queuing time exceeds prefetch latency

### Simulator Accuracy

- Mean TTFT: 19.2% maximum deviation from production
- Throughput: 6.5% maximum deviation
- Hit rate: 2.0% maximum deviation

---

## 2. Tiered KV Caching Taxonomy

### 2.1 Architectural Approaches

#### PagedAttention (vLLM, 2023)
- Applies OS virtual memory paging to KV cache
- Partitions KV cache into fixed-size blocks mapped via block tables
- Reduces memory waste from 60-80% to under 4%
- Up to 24x throughput vs HuggingFace Transformers
- Foundation for most modern serving frameworks

#### Multi-Tier Dynamic Storage (MTDS)
- Three-tier hierarchy: GPU HBM -> CPU DRAM -> NVMe SSD
- Block placement based on recency and predicted future use
- Layer-wise prefetching overlaps I/O with computation
- NVIDIA ICMSP (CES 2026): hardware-accelerated three-tier management

#### LMCache
- Enterprise-scale KV cache layer with GPU, DRAM, and disk tiers
- Selective KV cache reuse based on cache hit length
- Dynamic storage access control based on predicted hit probability

### 2.2 Eviction Policies

#### H2O: Heavy-Hitter Oracle (NeurIPS 2023)
- Observation: attention scores follow power-law distribution
- Small set of "heavy hitter" tokens receive disproportionate attention
- Policy: retain balance of recent tokens + heavy hitters (top accumulated attention)
- With 20% heavy hitters: up to 29x throughput improvement
- Limitation: local statistics may be sub-optimal for long contexts

#### StreamingLLM (ICLR 2024)
- Discovery: "attention sinks" -- initial tokens receive high attention regardless of content
- Policy: keep initial K tokens (attention sinks) + sliding window of recent tokens
- Enables infinite sequence length without fine-tuning
- Up to 22.2x speedup vs sliding window recomputation
- Limitation: loses mid-context information entirely

#### ScissorHands
- Predicts pivotal tokens with attention scores above per-window average
- Sequential prediction within history windows
- More adaptive than fixed-window approaches

#### FastGen
- Per-head eviction strategy selection from menu of policies:
  1. Locality (sliding window)
  2. Special tokens (StreamingLLM-style sinks)
  3. Frequency-based (H2O-style heavy hitters)
- Chooses best strategy per attention head during inference

#### SqueezeAttention
- 2D management: layer-wise + sequence-wise budget allocation
- 30-70% memory savings with maintained quality
- Recognizes different layers need different cache budgets

### 2.3 Compression Techniques

#### Quantization (INT4/INT8)
- Symmetric quantization: q = round(x / scale), scale = max(|x|) / qmax
- Per-block quantization for better accuracy
- INT8: 4x memory reduction, minimal accuracy loss
- INT4: higher compression, slight accuracy degradation
- NVIDIA NVFP4: hardware-accelerated on Blackwell GPUs

#### TurboQuant (Google Research, ICLR 2026)
- Two-stage approach: PolarQuant + QJL error correction
- 6x KV cache compression
- 8x attention computation speedup
- No calibration data required
- PolarQuant: angle-based quantization using atan2 with Remez-optimal polynomial
- QJL: Johnson-Lindenstrauss random projection for error estimation and correction

#### GQA / MQA (Attention Architecture)
- MQA: single shared K/V across all heads (10-100x cache reduction)
- GQA: G groups of shared K/V (H/G reduction factor)
- GQA reduces KV cache by up to 90% vs MHA
- 30-40% faster inference than MHA with near-equivalent accuracy
- Standard in Llama 2, Mistral, Mixtral, Gemma

### 2.4 Sparse Attention

#### Sliding Window Attention
- Cache only the most recent W tokens
- O(W) memory instead of O(N) for sequence length N
- Fails when relevant context exceeds window size

#### Sparse Attention Patterns
- Observation: most tokens contribute minimally to attention output
- Discard low-contribution tokens, approximate with sparser matrix
- Cerebras: 50% KV cache reduction via sparse attention

---

## 3. Mapping to Molt's 26 Tinygrad Primitives

### Available Primitives

The 26 tinygrad primitives available in molt:

**Unary**: NEG, EXP2, LOG2, SIN, SQRT, RECIPROCAL, TRUNC, CAST
**Binary**: ADD, SUB, MUL, IDIV, MOD, MAX, CMPLT, CMPEQ, CMPNE, AND, OR, XOR, SHL, SHR
**Ternary**: WHERE
**Reduce**: REDUCE_SUM, REDUCE_MAX
**Data**: CONST, LOAD

### Primitive Mapping for KV Cache Operations

| Operation | Primitives Used | Notes |
|-----------|----------------|-------|
| Attention score accumulation | MUL, REDUCE_SUM | dot product Q*K^T |
| Score normalization (softmax) | EXP2, MUL, REDUCE_SUM, RECIPROCAL | online softmax |
| Heavy-hitter detection | REDUCE_MAX, CMPLT, WHERE | threshold comparison |
| Quantization (scale computation) | REDUCE_MAX, RECIPROCAL, MUL | max(abs(x)) / qmax |
| Quantization (round) | MUL, TRUNC | q = trunc(x / scale) |
| Clamping | MAX, NEG | max(x, -qmax); min via -max(-x,-qmax) |
| Dequantization | MUL (+ SUB for asymmetric) | q * scale |
| QJL projection | MUL, REDUCE_SUM (matmul) | random projection + back-projection |
| Tier selection (WHERE chains) | CMPLT, WHERE | GPU-safe conditional without control flow |
| Score decay (exponential) | MUL | multiplicative decay per step |
| Sliding window mask | CMPLT, WHERE, CONST | position-based masking |
| Attention sink detection | REDUCE_SUM, CMPLT | cumulative score threshold |

### Compositions for Core Algorithms

**Heavy-Hitter Score Update** (per decoding step):
```
score_new = score_old * decay + attn_score_current
```
Primitives: MUL (decay), ADD (accumulate) -- 2 ops per token per step.

**Tier Assignment** (GPU-safe WHERE chain):
```
is_hot = score > hot_threshold        # CMPLT
is_warm = score > warm_threshold      # CMPLT
tier = WHERE(is_hot, HOT, WHERE(is_warm, WARM, COLD))  # 2x WHERE
```
Primitives: 2x CMPLT, 2x WHERE, 2x CONST -- 6 ops total, fuses to 1 kernel.

**Symmetric Quantization** (for warm tier):
```
abs_max = REDUCE_MAX(abs(x))          # REDUCE_MAX after abs via MAX(x, NEG(x))
scale = abs_max * (1/qmax)            # MUL + CONST
q = TRUNC(x * RECIPROCAL(scale))      # RECIPROCAL, MUL, TRUNC
q_clamped = MAX(q, -qmax)             # MAX + CONST (lower bound)
q_clamped = NEG(MAX(NEG(q), -qmax))   # NEG, MAX, NEG (upper bound)
```
Primitives: MAX, NEG, REDUCE_MAX, MUL, CONST, RECIPROCAL, TRUNC -- 11 ops, fuses to 2-3 kernels.

---

## 4. Integration with TurboQuant

Molt's `turbo_quant.py` already implements:
- `symmetric_quantize(x, n_bits)` -- per-tensor symmetric quantization
- `block_quantize(x, block_size, n_bits)` -- per-block symmetric quantization
- `dequantize_symmetric(q, scale)` -- dequantization
- `qjl_error_correction(original, quantized, scale, n_projections)` -- QJL correction
- `matmul_q4(a_f16, b_int4, scales)` -- mixed-precision matmul

**KV cache warm tier pipeline**:
1. When a token's importance score drops below `hot_threshold`, quantize its K and V vectors
   using `block_quantize(kv, block_size=128, n_bits=8)` for INT8 warm tier
2. Store quantized KV + per-block scales in warm tier storage
3. On warm-tier attention hit: `dequantize_symmetric(q_kv, scales)` before dot product
4. For higher compression (cold tier preparation): use `block_quantize(..., n_bits=4)` with
   `qjl_error_correction` to maintain accuracy

**Key insight**: PolarQuant's atan2-based angle quantization is designed for weight matrices,
not KV cache entries. For KV cache, standard symmetric/block quantization is more appropriate
because KV vectors have different distribution characteristics (not naturally polar-structured).
We use PolarQuant's infrastructure (Remez polynomial, domain reduction) only if we need
sub-4-bit quantization where angle-based representation becomes advantageous.

---

## 5. Integration with DDTree/DFlash

### Speculative Decoding KV Cache Compaction

After tree-walk verification in speculative decoding:
1. `compact_kv_cache(k, v, accepted_indices)` already removes rejected branches
2. **New**: after compaction, score the remaining tokens for tier assignment
3. Tokens that were not attended to during verification get their scores decayed
4. Tokens below warm threshold are quantized and moved to warm tier
5. Tokens below cold threshold are offloaded

### DDTree Expert Routing + KV Cache

For MoE models with DDTree routing:
- Each expert maintains its own KV cache partition
- Expert-specific eviction: tokens routed to inactive experts decay faster
- DDTree's log-probability scores inform cache priority (higher routing probability = more likely to be reused)

---

## 6. Implementation Plan

### Files to Create/Modify

1. **NEW** `src/molt/stdlib/tinygrad/kv_cache.py` -- Tiered KV cache manager
   - `TieredKVCache` class with hot/warm/cold tiers
   - H2O-inspired eviction with attention sink preservation (StreamingLLM)
   - Integration with `turbo_quant` for warm tier compression
   - Sliding window guarantee for recent context
   - Deterministic scoring and tier transitions

2. **MODIFY** `src/molt/stdlib/tinygrad/speculative.py` -- Wire KV cache into generic speculative decoding helpers
   - After `speculative_decode` verification, compact and tier-assign KV cache
   - Add `speculative_decode_with_kv_cache` that manages tiers post-verification

3. **MODIFY** `src/molt/stdlib/tinygrad/tree_attention.py` -- Wire tiered KV into attention
   - `tiered_tree_attention` that handles hot/warm/cold tiers
   - Hot: standard attention
   - Warm: dequantize-then-attend
   - Cold: reconstruct from offloaded storage

4. **NEW** `src/molt/stdlib/tinygrad/tests/test_kv_cache.py` -- Comprehensive tests
   - Tier promotion/demotion correctness
   - Quantization roundtrip accuracy
   - Eviction policy correctness
   - Memory savings measurement
   - Integration with speculative decoding

### No New Rust Primitives Required

All operations compose from existing primitives:
- Scoring: MUL + ADD (exponential moving average)
- Tier selection: CMPLT + WHERE (branchless tier assignment)
- Quantization: existing turbo_quant compositions
- Cache management: LOAD + data manipulation (Python-level)

### Determinism Requirements

- Attention score accumulation uses exact arithmetic (no floating-point non-determinism)
- Tier thresholds are deterministic functions of accumulated scores
- QJL uses fixed seed for reproducible random projections
- Eviction order is deterministic given same score history
