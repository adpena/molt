# Whisper Speech-to-Text via molt/tinygrad

## Model Variants
| Variant | Params | Layers | Dim | Heads | ONNX Size |
|---------|--------|--------|-----|-------|-----------|
| tiny | 39M | 4+4 | 384 | 6 | ~150 MB |
| base | 74M | 6+6 | 512 | 8 | ~290 MB |
| small | 244M | 12+12 | 768 | 12 | ~970 MB |

## Op Decomposition
ALL Whisper ops map to our 26 primitives:
- Conv1d = conv2d(x.unsqueeze(2), w) (height=1)
- Multi-head attention = matmul + softmax + matmul
- Cross-attention = same with encoder KV
- FFN = matmul + gelu + matmul
- Layer norm = reduceMean + sub + sqrt + reciprocal + mul + add
- GELU = x * sigmoid(1.702 * x) (composed)

## Deployment Path
1. Export Whisper tiny to ONNX (openai/whisper)
2. Run through our ONNX interpreter (29 ops, all supported)
3. Compile to WASM via molt
4. Run in browser with WebGPU acceleration
5. Voice-to-text without any server
