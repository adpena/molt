# ONNX Runtime Web vs molt/tinygrad -- Browser Benchmark

## Overview

Side-by-side comparison of PaddleOCR text detection latency between
onnxruntime-web (the standard ONNX runtime for browsers) and the
molt/tinygrad pipeline (compiled through the molt compiler to WebGPU/WASM).

## Models Under Test

| Model | File | Size | Ops |
|-------|------|------|-----|
| PaddleOCR v4 Detector (DBNet) | ch_PP-OCRv4_det.onnx | 4.7 MB | 342 constants |
| PaddleOCR v4 Recognizer (SVTRv2) | ch_PP-OCRv4_rec.onnx | 10.8 MB | 406 constants |

## Setup

### Prerequisites

- Chrome 120+ with WebGPU enabled (chrome://flags/#enable-unsafe-webgpu)
- GPU with Vulkan or Metal support
- Models uploaded to R2 at `models/paddleocr/`

### onnxruntime-web Backend

1. Open `/deploy/browser/bench-onnxrt.html` in Chrome
2. The page loads onnxruntime-web 1.17.0 from CDN
3. Models are fetched from R2 on first load (cached in IndexedDB)
4. Select WebGPU execution provider (falls back to WASM if unavailable)

### molt/tinygrad Backend

1. Open `/deploy/browser/test.html` in Chrome
2. The Falcon-OCR loader initializes the WebGPU compute engine
3. PaddleOCR weights are loaded and compiled through the tinygrad graph
4. Enable "Speculative decoding" toggle for recognition speedup

## Running the Comparison

1. Open both pages in separate Chrome tabs
2. Use the same test image (640x480 invoice recommended)
3. Upload to bench-onnxrt.html, note timing in the output panel
4. Upload to test.html, note timing in the output panel
5. Repeat 5 times, discard first run (cold start), average remaining 4

## Metrics Collected

| Metric | Description |
|--------|-------------|
| Cold start (ms) | Time from page load to first inference ready |
| Detection (ms) | DBNet forward pass only |
| Recognition (ms) | SVTRv2 forward pass per text region |
| Total pipeline (ms) | End-to-end: image in -> text out |
| Peak memory (MB) | GPU + JS heap via `performance.measureUserAgentSpecificMemory()` |
| Binary size (KB) | Total downloaded JS + WASM |

## Expected Results

| Metric | onnxruntime-web (WebGPU) | molt/tinygrad (WebGPU) | Notes |
|--------|--------------------------|------------------------|-------|
| Cold start | ~800 ms | ~600 ms | ort loads larger WASM runtime |
| Detection | ~5 ms | target < 5 ms | DBNet is conv-heavy |
| Recognition | ~3 ms/region | target < 3 ms/region | SVTRv2 is attention-heavy |
| Total | ~15 ms | target < 12 ms | Including CTC decode |
| Peak memory | ~45 MB | target < 35 MB | Fused ops reduce intermediates |
| JS + WASM | ~2.1 MB | target < 800 KB | Tree-shaked, no unused ops |

## molt/tinygrad Advantages

- **Smaller binary**: tree-shaking removes unused ONNX ops at compile time
- **Fused ops**: conv+bn+relu fused into single GPU dispatches
- **Custom memory**: arena allocator avoids GC pressure
- **WebGPU-native**: compute shaders, not translated from WebNN/DirectML

## Reproducing Server-Side

The same comparison can be run server-side via the Worker:

```bash
# molt/tinygrad ONNX JS interpreter (server-side, CPU)
curl -X POST https://falcon-ocr.adpena.workers.dev/ocr/paddle-molt \
  -H "Content-Type: application/json" \
  -d '{"image": "<base64>", "format": "image/png"}'

# Response includes timing breakdown:
# { "timing": { "detection_ms": ..., "total_ms": ... } }
```
