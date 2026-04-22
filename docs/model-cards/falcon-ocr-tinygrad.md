---
language: en
license: apache-2.0
library_name: molt
tags:
  - ocr
  - tinygrad
  - wasm
  - webgpu
  - falcon-ocr
  - vlm
  - compiled
  - quantized
pipeline_tag: image-to-text
---

# Falcon-OCR via molt/tinygrad

Heavy-duty 300M+ parameter vision-language model for OCR, compiled to
WebAssembly and WebGPU via [molt](https://github.com/aspect-build/molt).

## Overview

Falcon-OCR is a 300M+ parameter multimodal transformer (22 layers) that handles
complex OCR tasks: handwriting, multi-page documents, creative layouts, and
mixed-content images. It runs entirely in the browser via WebGPU compute shaders,
with fallback to WASM SIMD.

The model is AOT-compiled through molt's tinygrad tensor pipeline: the full
transformer graph is lowered to WASM (or native, or LLVM IR) with no runtime
graph parsing.

## Architecture

- **Type**: Vision-language model (ViT encoder + autoregressive decoder)
- **Parameters**: 300M+
- **Layers**: 22 transformer layers
- **Attention**: Grouped-query attention (GQA) with RoPE
- **FFN**: SiLU-gated (gate/up/down projections)
- **Normalization**: RMSNorm
- **Image input**: 128x128 -> 16x16 patches -> embedding -> prepend `<|OCR_PLAIN|>` token
- **Decoding**: Autoregressive token generation

## Quantization

| Quantization | Model Size | Quality (CER) | Status |
|-------------|-----------|----------------|--------|
| FP32        | ~1.2 GB   | Baseline       | Reference only |
| INT8        | ~300 MB   | < 8%           | Target for production |
| INT4        | ~150 MB   | > 40%          | Insufficient for 300M params |

INT4 quantization produces unacceptable quality on a 300M parameter model --
the 4-bit representation introduces too much noise. INT8 (256 discrete levels,
~16x less quantization error) is the minimum viable quantization for OCR tasks.

GPU f32 fma() does NOT compensate for INT4 quantization noise -- the bottleneck
is the weight representation, not the compute precision.

## WebGPU Inference Pipeline

1. Image -> PNG decode -> resize 128x128 -> 16x16 patches -> normalize [-1,1]
2. Patch embedding (CPU, small table)
3. Prepend `<|OCR_PLAIN|>` token
4. For each of 22 transformer layers:
   - RMSNorm (GPU: fused reduce + scale)
   - QKV projection (GPU: batched matmul, 3 dispatches in 1 command buffer)
   - RoPE rotation (GPU: fused sin/cos)
   - Attention: Q*K^T -> scale -> mask -> softmax -> V (GPU: 3 matmuls + softmax)
   - Output projection (GPU: matmul)
   - Residual add (GPU: elementwise)
   - FFN: RMSNorm -> gate/up -> SiLU -> mul -> down (GPU: 3 matmuls + elementwise)
   - Residual add (GPU: elementwise)
5. Final RMSNorm -> output projection -> logits (GPU)
6. Argmax -> token ID (CPU)
7. Autoregressive repeat
8. Decode tokens -> text

### Compute Backends (priority order)

1. **WebGPU WGSL compute shaders** (10-100x faster than CPU)
2. **WebGL2 GLSL fragment shaders** (3-30x faster than CPU)
3. **WASM SIMD f32x4** (2-4x faster than scalar)
4. **Scalar JS** (baseline, last resort)

### GPU Dispatch Optimizations

- **Batched QKV**: Q, K, V projections in a single compute pass + single submit (saves ~44ms/token over 22 layers)
- **Zero readback**: all intermediates stay as GPUBuffer; only final logits read to CPU
- **Command encoder pipelining**: all ops for one layer submitted together

### Speculative Decoding (browser only)

- **Draft model**: first 4 layers (fast, low quality)
- **Target model**: all 22 layers (full quality)
- **Expected speedup**: 3-5x more tokens per GPU batch cycle
- High acceptance rate for common OCR tokens (words, punctuation)

## WASM Binary

- **Size**: 13.4 MB uncompressed (~4 MB gzipped)
- **Exports**: `init()`, `ocr_tokens()`
- **Runtime**: molt WASM runtime (WASI preview1)

## Memory Budget (browser)

| Component | Size | Notes |
|-----------|------|-------|
| INT8 weights | ~300 MB | 22 layers, all projections |
| KV cache | ~12 MB | seqLen=256, 22 layers |
| Activations | ~6 MB | Double-buffered per layer |
| Buffer pool | ~20 MB | Reusable GPU buffers |
| **Total GPU memory** | **~340 MB** | Within 4 GB WebGPU limit |

## Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| First token latency | < 200ms | WebGPU warm, weights cached |
| Tokens/second | > 30 tok/s | INT8, single image |
| Full invoice OCR | < 3s | ~80 tokens average |
| Cold start (no cache) | < 5s | Weight download + compile |

## Deployment Matrix

| Platform | Engine | Weights | Status |
|----------|--------|---------|--------|
| Browser (desktop) | WASM + WebGPU | INT8 (300 MB) | Primary target |
| Browser (mobile) | WASM + WebGPU | INT4 (150 MB) | Best-effort |
| Cloudflare Workers | Workers AI (GPU fleet) | Remote | Production |
| Self-hosted (Node) | WASM or native | FP32/INT8 | Fully supported |

## Usage

```javascript
import { FalconOCR } from '@molt/falcon-ocr-browser';

const ocr = new FalconOCR({
  wasmUrl: '/falcon-ocr.wasm',
  weightsUrl: '/model-int8.safetensors',
  configUrl: '/config.json',
  quantization: 'int8',
});

await ocr.init();

const result = await ocr.run(width, height, rgbBytes, {
  maxTokens: 128,
  onToken: (token) => console.log(token),
});
console.log(result.text);
```

## Comparison with PaddleOCR-molt

| Aspect | Falcon-OCR | PaddleOCR-molt |
|--------|-----------|----------------|
| Parameters | 300M+ | ~15M (det + rec) |
| Binary size | 13.4 MB WASM | 10.8 MB WASM |
| Weight size | 300 MB (INT8) | 15.5 MB |
| Startup | ~5s (cold) | ~12 ms |
| Accuracy | High (complex layouts) | High (standard text) |
| Use case | Handwriting, creative layouts | Standard documents, invoices |
| GPU kernels | 7 (WebGPU WGSL) | 7 (shared engine) |
| Quantization | INT8 required | Not quantized (FP32 ONNX) |

PaddleOCR-molt is the fast path (15 ms cold start, 15.5 MB total payload).
Falcon-OCR is the quality fallback for edge cases that PaddleOCR cannot handle.

## License

Apache 2.0.
