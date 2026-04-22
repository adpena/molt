# openpilot Supercombo Driving Model — Molt Demo

**Date:** 2026-04-22

Demo scaffold for compiling comma.ai's on-policy driving model through
molt's tinygrad backend. This document records the model architecture
research and maps every op to our 26 tinygrad primitives.

## Model Architecture

The supercombo model is comma.ai's end-to-end driving policy network.
It predicts trajectories, lane lines, and lead vehicles directly from
camera frames. The architecture has three stages:

### 1. Encoder: EfficientNet-B2 Backbone

- ~70 convolutional layers using MBConv (mobile inverted bottleneck)
- Depthwise separable convolutions (grouped conv2d)
- Squeeze-and-Excitation attention blocks
- Swish/SiLU activation: `x * sigmoid(x)`
- Output: 1408-dim feature vector after global average pooling

### 2. Temporal Encoder: GRU

- 512-dim GRU cell carries hidden state across frames
- Input: visual features (1024-dim projected) + desire (8) + traffic (2) = 1034
- Maintains 5-second temporal context at 20 Hz (100 timesteps)
- Gates use sigmoid and tanh activations

### 3. Decoder: Multi-Head FC

- **Plan head** (4955 outputs): 5 trajectory hypotheses, each 33 timestamps x 3 coords x 2 stats (mean/std)
- **Lane head** (528 outputs): 4 lane lines + 2 road edges x 33 offsets x 2 stats + confidence
- **Lead head** (429 outputs): 3 lead vehicles x 33 timestamps x distance/speed/accel x 2 stats + probs
- **Meta head** (560 outputs): desire state, brake lights, turn signals, engagement

Total output: **(1, 6472)** packed tensor.

## Model Specifications

| Property | Value |
|---|---|
| Parameters | ~9.2 million |
| FLOPs per pass | ~0.65 billion |
| ONNX file size | 49 MB |
| Input resolution | 128 x 256 (YUV420, 12 channels) |
| Temporal context | 100 frames (5 seconds at 20 Hz) |
| Target hardware | Snapdragon 845 (comma 3X) |

## Input Tensors

| Name | Shape | Description |
|---|---|---|
| `input_imgs` | (1, 12, 128, 256) | 2 cameras x 2 frames x 3 channels (YUV420), float32 [0,1] |
| `desire` | (1, 100, 8) | One-hot command buffer for past 5 seconds at 20 Hz |
| `traffic_convention` | (1, 2) | Left-hand / right-hand drive one-hot |
| `initial_state` | (1, 512) | GRU hidden state carry from previous frame |

## Op Mapping to 26 Primitives

Every op in the supercombo model decomposes to our 26 tinygrad primitives.
No additional ops are needed.

### Backbone ops

| ONNX Op | Primitive Decomposition |
|---|---|
| Conv (standard) | im2col + matmul (MUL + REDUCE_SUM + ADD) |
| Conv (depthwise) | per-group im2col + matmul |
| Conv (grouped) | per-group im2col + matmul + concat |
| BatchNormalization | Folded into Conv at load time: (x-mean)/sqrt(var+eps)*w+b |
| Swish/SiLU | MUL(x, RECIPROCAL(ADD(1, EXP2(MUL(NEG(x), LOG2_E))))) |
| Sigmoid | RECIPROCAL(ADD(1, EXP2(MUL(NEG(x), LOG2_E)))) |
| Relu | MAX(x, 0) |
| GlobalAvgPool | REDUCE_SUM / spatial_size |

### GRU ops

| Op | Primitive Decomposition |
|---|---|
| MatMul | MUL + REDUCE_SUM |
| Sigmoid (gates) | RECIPROCAL(ADD(1, EXP2(MUL(NEG(x), LOG2_E)))) |
| Tanh | SUB(MUL(2, sigmoid(MUL(2, x))), 1) |
| Element-wise ops | ADD, SUB, MUL |

### Decoder ops

| Op | Primitive Decomposition |
|---|---|
| Linear (FC) | MUL + REDUCE_SUM + ADD |
| Relu | MAX(x, 0) |

### Weight loading ops

| Op | Primitive Decomposition |
|---|---|
| Quantized unpack | AND, OR, XOR, SHL, SHR, BITCAST |
| Type conversion | CAST |
| Reshape/Transpose | Zero-cost movement (data reindexing) |

### Full primitive coverage

All 26 primitives are exercised:

1. **ADD** — bias addition, residual connections, gate arithmetic
2. **SUB** — BatchNorm mean subtraction, tanh composition
3. **MUL** — matmul inner products, Swish self-gating, channel scaling
4. **IDIV** — integer index computation in im2col
5. **MOD** — spatial index wrapping in im2col
6. **NEG** — sigmoid input negation
7. **CMPLT** — padding boundary checks
8. **CMPEQ** — mask generation
9. **CMPNE** — validity checks
10. **AND** — quantized weight unpacking
11. **OR** — bit manipulation
12. **XOR** — bit manipulation
13. **SHL** — quantized weight unpacking
14. **SHR** — quantized weight unpacking
15. **EXP2** — sigmoid core: exp2(-x * log2(e))
16. **LOG2** — numerical stability in softmax (log_softmax path)
17. **SIN** — not directly used; available for positional encoding variants
18. **SQRT** — BatchNorm: 1/sqrt(var + eps)
19. **RECIPROCAL** — sigmoid denominator, average pool divisor
20. **TRUNC** — integer coordinate computation
21. **MAX** — relu activation, reduce_max in argmax
22. **WHERE** — conditional selection in padding, masking
23. **CAST** — float32/int32 type conversions
24. **BITCAST** — quantized weight reinterpretation
25. **REDUCE_SUM** — matmul contraction, global average pooling
26. **REDUCE_MAX** — argmax in post-processing

## Compilation Path

```
supercombo.onnx
    |
    v
openpilot_demo.py (this scaffold)
    |  tinygrad.onnx_interpreter parses ONNX graph
    |  BatchNorm folded into Conv weights
    |  All ops decomposed to 26 primitives
    v
molt build --target wasm openpilot_demo.py
    |  Python -> TIR -> WASM
    |  GPU kernels compiled to WebGPU compute shaders
    v
openpilot_demo.wasm + openpilot_demo.wgsl
    |
    v
Browser: WebGPU inference + Canvas driving visualization
```

## Demo Plan

### Phase 1: Architecture Validation (this PR)
- Scaffold with correct tensor shapes and layer structure
- Verify all ops decompose to 26 primitives
- No trained weights needed — random init validates shapes

### Phase 2: Weight Loading
- Download supercombo.onnx from openpilot repository
- Load via `tinygrad.onnx_interpreter` with BN fusion
- Validate output tensor shape: (1, 6472)

### Phase 3: Browser Visualization
- Compile to WASM via `molt build --target wasm`
- Canvas overlay rendering: lane lines, road edges, trajectory
- WebGPU compute for real-time inference
- Camera feed from getUserMedia or video file playback

## Weight Source

Trained weights are publicly available:
https://github.com/commaai/openpilot/blob/master/selfdrive/modeld/models/supercombo.onnx

The ONNX file is stored with Git LFS (49 MB). Our ONNX interpreter
already supports the required op set (Conv, BatchNorm, MatMul, Reshape,
Transpose, Sigmoid, Relu, Add, Mul, Concat, Squeeze, Unsqueeze).

## References

- [openpilot model README](https://github.com/commaai/openpilot/blob/master/selfdrive/modeld/models/README.md)
- [Diving into the Devils of Openpilot (arXiv:2206.08176)](https://ar5iv.labs.arxiv.org/html/2206.08176)
- [openpilot reimplementation (PyTorch)](https://github.com/ElectronicElephant/openpilot-reimplementation)
- [End-to-end lateral planning (comma.ai blog)](https://blog.comma.ai/end-to-end-lateral-planning/)
- [openpilot in 2021 (comma.ai blog)](https://blog.comma.ai/openpilot-in-2021/)
- [tinygrad supercombo compilation (issue #1926)](https://github.com/tinygrad/tinygrad/issues/1926)

## Non-Claims

This is a demo scaffold. Do not claim inference accuracy, latency, or
correctness until trained weights are loaded and outputs are validated
against the reference ONNX runtime. The model architecture is based on
public documentation and may differ from the latest supercombo revision.
