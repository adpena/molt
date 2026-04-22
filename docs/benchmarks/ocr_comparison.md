# OCR Engine Comparison

Benchmark date: 2026-04-14
Platform: macOS aarch64 (Apple Silicon), ONNX Runtime 1.x, PaddleOCR v4

## Detection Accuracy

| Engine | Avg Latency | Regions Found | Word Accuracy | Notes |
|--------|------------|---------------|---------------|-------|
| PaddleOCR ONNX Runtime | 37 ms | 21/21 (100%) | 43/44 (98%) | Baseline — local CPU inference |
| PaddleOCR molt/tinygrad | TBD | TBD | Target: match | WASM-compiled via molt |
| Falcon-OCR (Workers AI) | ~2-4 s | Variable | Hallucinated | Gemma 3 12B, non-deterministic |
| Falcon-OCR (WebGPU) | TBD | TBD | Expected: good | Browser-side, pending WebGPU backend |

## Full Pipeline (Detect + Recognize)

| Engine | Detect | Recognize | Total | Quality |
|--------|--------|-----------|-------|---------|
| PaddleOCR ONNX Runtime (CPU) | 30-48 ms | included | 30-48 ms | 98% word accuracy |
| PaddleOCR molt/tinygrad (WASM) | ~11 ms startup | included | ~11 ms + inference | Match ONNX baseline |

## WASM Performance (2026-04-14)

| Metric | Value |
|--------|-------|
| WASM binary size | 10.8 MB |
| WASM instantiate | 11.0 ms (Node.js, Apple Silicon) |
| Conv+Activation fusion | 62 nodes fused |
| PaddleOCR exports | init, init_full, ocr, detect_only, rgb_bytes_to_tensor |
| Chinese OCR (你好世界) | 你好世界 -- correct |
| WebGPU Conv2d kernel | Direct convolution, 16x16 workgroup, fma()-optimized |

## Per-Invoice Breakdown (ONNX Runtime)

| Invoice Type | Lines | Latency | Regions | Accuracy |
|-------------|-------|---------|---------|----------|
| Simple (invoice header) | 3 | 30 ms | 3 | 97% (1 leading char clipped) |
| Detailed (receipt) | 6 | 48 ms | 6 | 100% |
| Numbers (order) | 4 | 35 ms | 4 | 100% |
| Mixed (address) | 4 | 35 ms | 4 | 100% |
| Currency (multi-currency) | 4 | 37 ms | 4 | 100% |

## Test Methodology

- 5 synthetic invoice images generated with Pillow (600x variable px, black text on white)
- Font: system Helvetica 20pt on macOS
- Detection: PaddleOCR v4 det model, threshold 0.3, contour extraction
- Recognition: PaddleOCR v4 English rec model, CTC greedy decode
- Accuracy: word-level substring match against ground truth (case-insensitive)
- Run: `.venv/bin/python` inline benchmark (see task 3 in e2e hardening script)

## R2 Asset Verification

All 12 production assets verified accessible (2026-04-14):

| Asset | Path | Status |
|-------|------|--------|
| WASM: falcon-ocr | /wasm/falcon-ocr.wasm | 200 |
| WASM: paddleocr | /wasm/paddleocr.wasm | 200 |
| Model: det | /models/paddleocr/ch_PP-OCRv4_det.onnx | 200 |
| Model: rec | /models/paddleocr/ch_PP-OCRv4_rec.onnx | 200 |
| Dict: English | /models/paddleocr/dicts/en_ppocr_dict.txt | 200 |
| Dict: Japanese | /models/paddleocr/dicts/japan_dict.txt | 200 |
| Dict: Korean | /models/paddleocr/dicts/korean_dict.txt | 200 |
| Config: int8 | /weights/falcon-ocr-int8/config.json | 200 |
| Tokenizer | /tokenizer.json | 200 |
| JS: compute | /browser/compute-engine.js | 200 |
| JS: webgpu | /browser/webgpu-engine.js | 200 |
| WASM: simd-ops | /browser/simd-ops.wasm | 200 |

## Endpoint Verification

| Endpoint | Method | Status |
|----------|--------|--------|
| /test | GET | 200 |
| /test/paddle | GET | 200 |
| /dashboard | GET | 200 |
| /health | GET | 503 (model not loaded) / 200 (model ready) |
