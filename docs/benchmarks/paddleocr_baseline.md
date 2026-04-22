# PaddleOCR v4 ONNX Runtime Baseline

**Date:** 2026-04-21
**Platform:** macOS aarch64 (Apple Silicon), Python 3.12, onnxruntime 1.24.4

## Results

| Stage | Time (ms) | Notes |
|-------|-----------|-------|
| Detection (DBNet) | 16.4 | 640x480 image, 62 conv layers |
| Recognition (SVTRv2) | 4.5 | Single crop, 48px height |
| Full pipeline | 41.5 | Detect + contour extraction + 3 crops + recognize |

## Model sizes

| Component | ONNX Size | Nodes | Op Types |
|-----------|-----------|-------|----------|
| Detector (ch_PP-OCRv4_det) | 4.7 MB | 778 | 15 |
| Recognizer (ch_PP-OCRv4_rec) | 10.8 MB | 934 | 26 |
| Classifier | 0.6 MB | 566 | — |
| Total | 16.1 MB | — | — |

## Baseline targets for molt/tinygrad

| Metric | ONNX Runtime | Target (molt) |
|--------|-------------|---------------|
| Detection | 16.4 ms | < 15 ms |
| Recognition | 4.5 ms | < 4 ms |
| Full pipeline | 41.5 ms | < 35 ms |
| Binary size | 16.1 MB (ONNX) | < 3 MB (WASM gzipped) |
| Startup | ~100 ms (ONNX load) | < 50 ms (compiled) |

## How molt can beat ONNX Runtime

1. **AOT compilation** — no runtime graph parsing, no dynamic dispatch
2. **Kernel fusion** — fuse BatchNorm into Conv (compile-time constant folding)
3. **WebGPU dispatch** — 62 conv ops dispatched to GPU compute shaders
4. **Tree-shaking** — only include the 15 op types actually used
5. **SIMD matmul** — our 10.3 us matmul vs onnxruntime's generic dispatcher
