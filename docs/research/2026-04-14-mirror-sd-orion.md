# Mirror Speculative Decoding + Orion ANE: Research Synthesis for Molt GPU Stack

**Date**: 2026-04-14
**Papers**: arxiv 2510.13161 (Mirror-SD), arxiv 2603.06728 (Orion)
**Scope**: Implementation plan for heterogeneous GPU+ANE inference on Apple Silicon

---

## 1. Mirror Speculative Decoding (Bhendawade et al., Apple, 2025)

### 1.1 Problem Statement

Standard speculative decoding suffers from a fundamental latency-acceptance tradeoff:
the draft model runs serially before the target model, so increasing draft capacity
(larger gamma, deeper draft) improves acceptance rate rho but adds proportional latency.
The step latency is:

```
T_SD(gamma; phi, theta) = T_draft(gamma; phi) + T_target(gamma; theta)   [Eq. 5]
```

This serial dependency means improvements in acceptance must compensate for added
draft latency, intrinsically coupling acceptance with wall-time cost.

### 1.2 Core Algorithm: Parallel Draft-Target Execution

Mirror-SD breaks the serial barrier by overlapping draft and target execution on
heterogeneous devices (GPU for target, NPU for draft). The key insight: after the
target model passes through its early-exit layer l_e (typically at depth N/2), it
emits a lightweight token channel M_t containing the top-kappa candidates and their
log-probabilities:

```
M_t = Top-kappa(p^(l_e)(. | y_{<t}, x)) = {(v_i, log p_i)}_{i=1}^{kappa}   [Eq. 7]
```

This is a low-bandwidth message (O(B * kappa) token IDs + log-probs) sent to the
draft model while the target continues computing layers l_e+1 through L_N.

### 1.3 Branch-Complete Concurrent Speculation

Given M_t, the draft begins a branch-complete rollout in parallel: for each candidate
v_i in M_t and for every prefix length r <= gamma, it prepares a speculative
continuation for the next step:

```
For all i in {1,...,kappa}, for all r in {1,...,gamma}:
    y'^(i)_{t+1:t+r} ~ f_d(. | y_{<t}, x, y_{t+1} = v_i)   [Eq. 8]
```

This produces a hypothesis tree T_t with kappa roots and depth gamma.

### 1.4 Verification vs. Reuse Criterion

At step t, the target accepts a prefix of length A_t and issues a correction at
position tau = A_t + 1:

```
A_t = max{r in {0,...,gamma} : y_hat_{t+j} = y^targ_{t+j} for all j <= r}
```

If A_t < gamma, the correction token is:
```
c_{t+tau} = y^targ_{t+tau} ~ p^(N)(. | y_{<t+tau-1}, x)
```

The corrected prefix Pi_t^+ = (Pi_t, c_{t+tau}) is checked against precomputed
branches. **Reuse** occurs when this corrected prefix already appears as a path in
the hypothesis tree T_t, avoiding recomputation of the next draft window.

**Operational selection of the next window:**
- If A_t = 0 and correction matches a root: branch rooted at c_{t+1}
- If A_t >= 1 and Pi_t^+ in Paths_tau(T_t): precomputed continuation at depth tau
- Otherwise: fresh rollout from (y_{1:t+A_t}, c_{t+tau})

### 1.5 Speculative Streaming (SS) on Draft

The draft employs Speculative Streaming: it verifies previously proposed tokens while
generating new speculative tokens in the same forward pass using multi-stream
attention. A shared LM head W_LM^(d) projects both the main stream and lookahead
streams to token logits:

```
W_LM^(d): M_t^(N) -> p_d(. | h_t)
W_LM^(d): S_t^(N) -> p_d(. | h_t, j), j = 1,...,gamma
```

A single draft internal step can emit n_j >= 1 tokens. The number of draft steps
J required to materialize gamma tokens satisfies:

```
J <= ceil(gamma / eta_bar), where eta_bar = (1/J) * sum_{j=1}^{J} n_j
```

### 1.6 Step Latency Model

```
T_Mirror = T_target^{1:l_e} + T_rv^(ee) + max{T_target^{l_e+1:N}, T_draft^gen(gamma)} + T_rv^(fv)   [Eq. 10]
```

Where T_rv^(ee) and T_rv^(fv) are the early-exit and final-verification rendezvous
overheads (microsecond-scale token channel transfers).

**Overlap budget**: Delta = T_target^{l_e+1:N}. If T_draft^gen(gamma) <= Delta,
the entire draft generation is hidden under the target suffix, giving:
T_Mirror = T_target + T_rv (draft is free).

### 1.7 Heterogeneous Sharding

- **Target**: Megatron-style tensor parallelism across G_T GPUs (column-parallel
  W_qkv and W_o in MHA, column/row-parallel W_1, W_2 in MLP)
- **Draft**: SPD-style sharding across G_D NPUs (two contiguous segments, parallel
  tracks pinned to individual NPUs with resident weight shards)
- **Cross-accelerator rendezvous**: Two token-level exchanges per step carrying
  O(B * kappa) items (negligible vs millisecond compute)

### 1.8 Results

On SpecBench with Qwen3-14B/32B, Mistral-24B, OPT-66B:
- 2.8x-5.8x wall-time speedups across diverse tasks
- 30% average relative improvement over EAGLE3
- Mathematical reasoning: up to 5.84x on Qwen3-32B
- Gains most pronounced on long-horizon workloads

---

## 2. Orion: Direct ANE Programming (Kumaresan, 2026)

### 2.1 Architecture

Orion bypasses CoreML entirely via private Apple APIs loaded through dlopen/objc_getClass
from AppleNeuralEngine.framework:

| Class | Role |
|-------|------|
| _ANEClient | Singleton daemon connection |
| _ANECompiler | MIL -> E5 microcode compilation |
| _ANEInMemoryModel | In-memory program (no filesystem) |
| _ANEModel | Compiled program handle |
| _ANERequest | Evaluation request spec |
| _ANEIOSurfaceObject | IOSurface tensor wrapper |

### 2.2 The 20 ANE Constraints

**MIL IR Restrictions (6):**
1. `concat` op causes immediate compilation failure; must decompose into separate programs
2. SDPA causal masks silently ignored; requires manual masking
3. `gelu` activation unsupported; requires tanh approximation decomposition
4. matmul transpose flags need explicit named constants (inline literals rejected)
5. `conv` op lacks bias parameter; separate `add` operation required
6. 32K-channel convolutions rejected by compiler

**Memory & I/O Constraints (9):**
7. Multi-output buffers require uniform byte sizes across all outputs
8. Outputs ordered alphabetically by MIL variable names (not tuple order)
9. Minimum ~49 KB IOSurface allocation (pads single-token tensors to [1,768,1,16])
10. BLOBFILE weight offset is 64 bytes from chunk header (not file start)
11. MIL text requires NSData* encoding, not NSString*
12. Weight dictionary must be @{} empty dict, never nil
13. Multi-input surfaces need uniform allocation sizes
14. Input surfaces ordered alphabetically by parameter name
15. ANE reads flat buffers as packed [1,C,1,S] from byte 0

**Compilation Limits (4):**
16. ~119 compilations per process before silent failures
17. Weights baked at compile time; immutable post-compilation
18. Output variables must reference live post-optimization nodes
19. exec() restart overhead ~50 ms

**Performance Characteristics (1):**
20. 32 MB SRAM performance cliff causes ~30% degradation when exceeded

### 2.3 MIL Compiler Pipeline

Five-pass optimization loop (maximum 20 iterations):

1. **Dead Code Elimination**: backward reachability walk from outputs
2. **Identity Elimination**: removes no-op casts, reshapes, transpositions
3. **Cast Fusion**: eliminates round-trip casts (fp16->fp32->fp16)
4. **SRAM Annotation**: estimates working-set against 32 MB budget
5. **ANE Constraint Validation**: checks banned ops, tensor sizes, liveness

**Supported MIL Operations (27):**

| Category | Operations |
|----------|-----------|
| Data | input, const, identity |
| Linear | conv1x1, matmul |
| Elementwise | add, sub, mul, neg |
| Activation | relu, tanh, sigmoid |
| Math | exp, pow, sqrt, rsqrt |
| Reduction | sum, mean, max |
| Shape | reshape, transpose, split, pad, slice |
| Other | cast, softmax |

### 2.4 Tensor Layout and IOSurface Zero-Copy

All ANE I/O uses fp16 in [1, C, 1, S] format on IOSurface-backed shared memory.
Runtime handles transpose between CPU [seq, d_model] and ANE [1, d_model, 1, seq].

IOSurface provides zero-copy CPU-to-ANE transfer: the same physical memory is
accessible from both CPU and ANE without copying. This maps directly to our
DeviceBuffer abstraction with a new `BufferHandle::Ane(IOSurfaceRef)` variant.

### 2.5 Performance

**Inference (GPT-2 124M on M4 Max):**
- ANE decode: 170 tokens/sec
- ANE prefill (cached): 165 tok/s
- CPU decode baseline: 283 tok/s
- Softmax (vocab=32000): 33.8x faster on ANE than CPU
- Classifier forward: 10.2x faster on ANE

**Training (Stories110M, M4 Max):**
- Delta compilation: 494 ms vs 4,200 ms full recompile (8.5x speedup)
- Training: 22.4 min for 1,000 steps with delta reload
- Stable: 0/1,000 NaN occurrences with numerical fixes

### 2.6 CPU/ANE Division of Labor

| Operation | Device | Reason |
|-----------|--------|--------|
| Transformer fwd/backward | ANE | Compute-bound convolutions |
| Token sampling | CPU | Sequential/branching logic |
| Adam optimizer | CPU | Weights immutable on ANE |
| Embedding lookup | CPU | Table indexing |
| NLL loss + gradient | CPU | gather not in MIL |
| Classifier backward | CPU | 32K channels rejected |

### 2.7 Delta Compilation for Weight Updates

Eliminates weight-update recompilation bottleneck:
1. Unload existing _ANEModel via `unloadWithQoS(21)`
2. Write updated weight BLOBFILEs to disk
3. Reload via `loadWithQoS(21)` (bypasses ANECCompile entirely)

Relevant for Falcon-OCR weight loading: pre-compiled ANE programs can hot-swap
weights without the ~1 second compilation penalty.

---

## 3. Synthesis: Apple Silicon Heterogeneous Inference

### 3.1 Combined Architecture

Mirror-SD needs two devices. Orion gives us direct ANE access. Together:

```
                Metal GPU                          ANE (Neural Engine)
            +-------------------+              +-------------------+
            | Target Model      |              | Draft Model       |
            | (full precision)  |              | (fp16, conv1x1)   |
            |                   |              |                   |
            | Layer 0..l_e      |--- M_t ----->| Branch-Complete   |
            | (early-exit)      | (top-kappa   | Rollout           |
            |                   |  tokens)     |                   |
            | Layer l_e+1..N    |              | SS Multi-Token    |
            | (verification)    |<--- draft ---| Streaming         |
            +-------------------+  candidates  +-------------------+
                     |                                    |
                     +--------- IOSurface ----------------+
                               (zero-copy shared memory)
```

**Why this mapping works:**
1. Target model on GPU: high-fidelity, full-precision, complex ops (gather, scatter,
   arbitrary activations)
2. Draft model on ANE: conv1x1-heavy, fp16, high throughput (19 TFLOPS), low latency
3. Token channel M_t: O(B * kappa) items = microsecond transfer via shared memory
4. IOSurface zero-copy: DeviceBuffer shared between Metal and ANE without memcpy

### 3.2 Mapping to Molt's 26 Primitives

**ANE-native mapping (17/26 primitives):**

| Primitive | MIL Equivalent | ANE Support |
|-----------|---------------|-------------|
| Add | add | Native |
| Sub | sub | Native |
| Mul | mul | Native |
| Neg | neg | Native |
| Exp2 | exp (via exp2 = exp(x * ln2)) | Native (decompose) |
| Log2 | pow + log decomposition | Native (decompose) |
| Sin | Not in MIL | **Metal fallback** |
| Sqrt | sqrt | Native |
| Reciprocal | rsqrt(x*x) or mul(1/x) | Native (decompose) |
| Trunc | cast(cast(x, int), float) | Native (decompose) |
| Max | max (reduction) | Native |
| Where | select via mul+add | Native (decompose) |
| Cast | cast | Native |
| ReduceSum | sum | Native |
| ReduceMax | max | Native |
| Cmplt | sub + relu + sign decomposition | Native (decompose) |
| Cmpeq | sub + abs + threshold | Native (decompose) |

**Metal-only (9/26 primitives):**

| Primitive | Reason for Metal Fallback |
|-----------|--------------------------|
| Idiv | Integer division not in MIL |
| Mod | Modulo not in MIL |
| Cmpne | Complex decomposition overhead |
| And | Bitwise not in MIL |
| Or | Bitwise not in MIL |
| Xor | Bitwise not in MIL |
| Shl | Bitwise not in MIL |
| Shr | Bitwise not in MIL |
| Bitcast | Reinterpret not in MIL |
| Sin | Trig not in MIL |

**Implication**: The draft model for Mirror-SD should be designed to avoid bitwise
and integer ops. LLM draft models are predominantly float matmul + activation +
normalization, which maps well to ANE-supported ops.

### 3.3 ANE Constraints Impact on Kernel Generation

Critical constraints for our MIL renderer:

1. **No concat**: Multi-head attention must use separate programs per head or
   conv1x1 with channel grouping instead of concat
2. **32 MB SRAM cliff**: Working set per kernel must stay under 32 MB or suffer
   30% degradation. Our FusedKernel scheduler must estimate working set.
3. **fp16 only**: All ANE computation is fp16. Draft model must be quantized.
4. **[1,C,1,S] layout**: Our ShapeTracker must emit ANE-compatible strides.
5. **~119 compilation limit**: Must cache aggressively and use delta compilation
   for weight updates.
6. **conv1x1 >> matmul**: Prefer conv1x1 emission over matmul (3x throughput).
7. **No bias in conv**: Must emit separate add for bias terms.
8. **Alphabetical output ordering**: IOSurface outputs ordered by MIL variable name,
   not by logical output index. Must name variables carefully.

### 3.4 Performance Projections

**Draft model on ANE (M4 Max, fp16):**
- Conv1x1 throughput: ~19 TFLOPS
- Draft generation for gamma=5 tokens: ~0.5 ms (hidden under target suffix)
- Branch-complete rollout (kappa=4 roots, gamma=5 depth): ~2 ms

**Target model on Metal GPU (M4 Max, fp32/fp16):**
- Early-exit at l_e = N/2: ~5 ms for first half
- Suffix l_e+1 to N: ~5 ms (overlap budget Delta)
- Total target: ~10 ms per step

**Combined Mirror-SD:**
- T_Mirror = 5 ms (prefix) + max(5 ms, 2 ms) + ~0.01 ms (rendezvous) = ~10 ms
- Draft is fully hidden: 0 ms marginal cost
- Effective: ~10 ms per step generating 3-5 accepted tokens
- Throughput: 300-500 tokens/sec for small models on M4 Max

**Comparison:**
- Standard SD: ~15-20 ms (draft 5-10 ms + target 10 ms)
- Mirror-SD: ~10 ms (draft hidden) = 1.5-2x improvement
- With SS multi-token streaming: potentially 2-3x over standard SD

---

## 4. Implementation Architecture

### 4.1 ANE Device Backend (`runtime/molt-gpu/src/device/ane.rs`)

```
AneDevice
  |- Allocator: IOSurface-backed buffers, [1,C,1,S] layout
  |- Compiler: MIL text -> _ANECompiler -> _ANEModel
  |- Executor: _ANERequest dispatch, IOSurface I/O
  |- Cache: program cache with composite keys, delta compilation
```

Feature-gated behind `ane-backend`, macOS-only.

### 4.2 MIL Renderer (`runtime/molt-gpu/src/render/mil.rs`)

Generates MIL text IR from FusedKernel. Maps 17/26 primitives natively, marks
9 as requiring Metal fallback. Handles:
- conv1x1 preference over matmul
- Bias as separate add
- fp16 type narrowing
- [1,C,1,S] shape annotation
- SRAM budget estimation
- Alphabetical variable naming

### 4.3 Mirror-SD Python (`src/molt/stdlib/tinygrad/mirror_sd.py`)

Pure Python implementation of the Mirror-SD algorithm:
- Early-exit proxy distribution extraction
- Branch-complete rollout with hypothesis tree
- Verification with reuse criterion
- Speculative streaming on draft
- Heterogeneous device scheduling

### 4.4 Heterogeneous Device Manager

Extends `device.py` with multi-device scheduling:
- Device capability detection (GPU ops, ANE ops)
- Op routing based on primitive support
- IOSurface zero-copy buffer sharing
- Token channel for cross-device communication

---

## References

1. Bhendawade, N. et al. "Mirror Speculative Decoding: Breaking the Serial Barrier
   in LLM Inference." arXiv:2510.13161, Apple, 2025.
2. Kumaresan, R. "Orion: Characterizing and Programming Apple's Neural Engine for
   LLM Training and Inference." arXiv:2603.06728, 2026.
3. GitHub: mechramc/Orion (MIT license, Murai Labs)
4. Apple Machine Learning Research: machinelearning.apple.com/research/mirror
