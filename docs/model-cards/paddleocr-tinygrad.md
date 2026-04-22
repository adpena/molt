---
language: en
license: apache-2.0
library_name: molt
tags:
  - ocr
  - tinygrad
  - wasm
  - webgpu
  - paddleocr
  - compiled
datasets:
  - paddleocr
pipeline_tag: image-to-text
---

# PaddleOCR via molt/tinygrad

The first AOT-compiled PaddleOCR using tinygrad tensor primitives.

## Overview

PaddleOCR v4 (detector + recognizer) reimplemented as compositions of
26 tinygrad primitives and compiled to WebAssembly via [molt](https://github.com/aspect-build/molt).

The ONNX computation graphs (778 nodes for the detector, 934 for the
recognizer) are walked by `OnnxInterpreter`, which decomposes every op
into tinygrad primitives. Molt's AOT compiler then lowers the entire
tensor program to WASM (or native, or LLVM IR).

## Features

- **Compiled inference** -- AOT compiled, no runtime graph parsing
- **WebGPU acceleration** -- 7 GPU compute shaders for Conv2d, matmul, softmax, RMSNorm, RoPE, add, mul
- **Graph optimizations** -- BatchNorm folding, Identity elimination, 62 Conv+Activation fusions
- **Multi-language** -- 11 language dictionaries (en, zh, ja, ko, latin, cyrillic, devanagari, arabic, chinese_cht, ppocrv5)
- **10.8 MB WASM binary** -- vs 15.5 MB raw ONNX models (detector + recognizer)
- **Browser-native** -- runs entirely client-side, no server needed
- **WASM SIMD** -- optional SIMD-accelerated matmul kernel (4.6 KB supplementary module)
- **Server-side ready** -- 15.5 MB total fits in Workers 256 MB memory

## Architecture

| Component   | Model          | ONNX Size | Nodes | Key Ops                        |
|-------------|----------------|-----------|-------|--------------------------------|
| Detector    | DBNet (PP-OCRv4) | 4.7 MB  | 778   | 62 Conv layers, 10 HardSigmoid |
| Recognizer  | SVTRv2         | 10.8 MB   | 934   | 6625-class CTC, MatMul-heavy   |
| Classifier  | Direction      | 0.6 MB    | ~50   | Lightweight MobileNet          |

### Graph Optimizations

The `OnnxInterpreter.optimize_graph()` pass applies before execution:

1. **BatchNorm folding** -- folds BN scale/bias/mean/var into preceding Conv weights (eliminates BN entirely)
2. **Identity elimination** -- removes Identity nodes, rewires edges
3. **Conv+Activation fusion** -- fuses Conv followed by Relu, HardSigmoid, or HardSwish into a single FusedConvActivation node (62 fusions in the detector, eliminating 62 kernel launches)

### WebGPU Kernels (7 total)

| Kernel | WGSL Entry Point | Used By |
|--------|-------------------|---------|
| Conv2d | `molt_conv2d` | Detector (62 layers, ~60% compute) |
| MatMul | `molt_kernel` | Recognizer attention + projections |
| Softmax | `molt_softmax` | Attention layers |
| RMSNorm | `molt_rms_norm` | Layer normalization |
| RoPE | `molt_rope` | Rotary position embeddings |
| Add | `molt_add` | Residual connections |
| Mul | `molt_mul` | Gating (SiLU, HardSwish) |

## Performance

| Metric              | ONNX Runtime (CPU) | molt/tinygrad (WASM) | molt/tinygrad (WebGPU) |
|---------------------|--------------------|----------------------|------------------------|
| Detection           | 4.8 ms             | TBD                  | TBD                    |
| Recognition         | 4.2 ms             | TBD                  | TBD                    |
| Binary size         | 15.5 MB            | 10.8 MB              | 10.8 MB                |
| WASM compile        | N/A                | 6.9 ms               | N/A                    |
| WASM instantiate    | N/A                | 5.0 ms               | N/A                    |
| WASM total startup  | N/A                | 12.2 ms              | N/A                    |
| Conv2d JS (64ch)    | N/A                | 5.6 ms (2.5 GFLOPS) | TBD                    |

WASM startup measured via Node.js WebAssembly.Module + Instance (M3 Pro, cold start).

### WASM Binary

- **Size**: 10.8 MB (uncompressed)
- **Functions**: 31 exported
- **PaddleOCR exports**: init, init_full, ocr, detect_only, rgb_bytes_to_tensor
- **Runtime exports**: molt_main, molt_alloc, molt_isolate_bootstrap, and 23 others
- **Total payload** (WASM + weights): 26.4 MB

### WASM Exports

The compiled module exports the full PaddleOCR inference pipeline:
- `tinygrad_paddleocr_driver__init` / `tinygrad_paddleocr_driver__init_full`
- `tinygrad_paddleocr_driver__detect_only`
- `tinygrad_paddleocr_driver__ocr`
- `tinygrad_paddleocr_driver___rgb_bytes_to_tensor`
- molt runtime functions: `molt_alloc`, `molt_main`, `molt_isolate_bootstrap`, etc.

## i18n Verification

Tested with ONNX Runtime + ch_PP-OCRv4_rec recognizer (6625 classes):

| Language | Input | Output | Status | Notes |
|----------|-------|--------|--------|-------|
| Chinese  | 你好世界 | 你好世界 | MATCH | ch_PP-OCRv4_rec covers Chinese natively |
| Japanese | こんにちは | (empty) | NEEDS ja MODEL | Chinese recognizer lacks hiragana/katakana |
| Korean   | 안녕하세요 | (empty) | NEEDS ko MODEL | Chinese recognizer lacks hangul |

Japanese and Korean require language-specific recognizer ONNX models
(ja_PP-OCRv4_rec, ko_PP-OCRv4_rec) which are not yet downloaded.
The dictionaries (japan_dict.txt: 4,399 chars, korean_dict.txt: 3,679 chars)
are available but the ch_PP-OCRv4_rec model only outputs from the Chinese
charset (6,625 classes). Full CJK support requires downloading the
language-specific recognizer weights from PaddleOCR upstream.

## Supported Languages

| Language    | Dictionary File        | Characters | Recognizer Verified |
|-------------|------------------------|------------|---------------------|
| English     | en_dict.txt            | 95         | Yes (via ch model)  |
| Chinese     | ppocr_keys_v1.txt      | 6,623      | Yes (MATCH)         |
| Japanese    | japan_dict.txt         | 4,399      | No (needs ja model) |
| Korean      | korean_dict.txt        | 3,679      | No (needs ko model) |
| Latin       | latin_dict.txt         | 130        | Pending             |
| Cyrillic    | cyrillic_dict.txt      | 116        | Pending             |
| Devanagari  | devanagari_dict.txt    | 143        | Pending             |
| Arabic      | arabic_dict.txt        | 117        | Pending             |
| Chinese Trad| chinese_cht_dict.txt   | 8,414      | Pending             |
| PPOCRv5     | ppocrv5_dict.txt       | 18,207     | Pending             |
| English+    | en_ppocr_dict.txt      | 388        | Pending             |

## Usage

```python
from tinygrad.onnx_interpreter import OnnxInterpreter
from tinygrad.tensor import Tensor

# Load detector
det = OnnxInterpreter()
det.load_model(open("ch_PP-OCRv4_det.onnx", "rb").read())

# Run detection
image = Tensor.load("input.bin")  # NCHW float32
det_output = det.run({"x": image})

# Load recognizer
rec = OnnxInterpreter()
rec.load_model(open("ch_PP-OCRv4_rec.onnx", "rb").read())
rec_output = rec.run({"x": crop_tensor})
```

## WASM Deployment

```javascript
// Browser usage (requires weight files)
const wasmBytes = await fetch("/paddleocr.wasm").then(r => r.arrayBuffer());
const { instance } = await WebAssembly.instantiate(wasmBytes, imports);

// Load weights into WASM memory
instance.exports.init(detWeights, recWeights, dictString);

// Run OCR on image
const result = instance.exports.ocr(imageBytes, width, height);
```

## Server-Side Deployment (Cloudflare Workers)

Models fit in Workers 256 MB memory:
- Detector: 4.7 MB
- Recognizer: 10.8 MB
- Dictionary: 74 KB
- Total: 15.5 MB

Models are loaded from R2 on first request and cached in global scope.
The `/ocr/paddle-molt` endpoint serves inference via the ONNX interpreter.

## Model Provenance

- Detector: [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) ch_PP-OCRv4_det
- Recognizer: [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) ch_PP-OCRv4_rec
- ONNX conversion: [OleehyO/paddleocrv4.onnx](https://huggingface.co/OleehyO/paddleocrv4.onnx)
- Dictionaries: PaddleOCR official ppocr/utils/dict/

## License

Apache 2.0 (same as PaddleOCR upstream).
