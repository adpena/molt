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
- **WebGPU acceleration** -- GPU compute shaders for Conv2d, matmul, softmax
- **Graph optimizations** -- BatchNorm folding, Identity elimination, Conv+Activation fusion
- **Multi-language** -- 11 language dictionaries (en, zh, ja, ko, latin, cyrillic, devanagari, arabic, chinese_cht, ppocrv5)
- **Tiny binary** -- 10.3 MB WASM (3.2 MB gzipped) vs 16 MB raw ONNX models
- **Browser-native** -- runs entirely client-side, no server needed
- **WASM SIMD** -- optional SIMD-accelerated matmul kernel (4.6 KB supplementary module)

## Architecture

| Component   | Model          | ONNX Size | Nodes | Key Ops                        |
|-------------|----------------|-----------|-------|--------------------------------|
| Detector    | DBNet (PP-OCRv4) | 4.7 MB  | 778   | 62 Conv layers, 10 HardSigmoid |
| Recognizer  | SVTRv2         | 10.8 MB   | 934   | 438-class CTC, MatMul-heavy    |
| Classifier  | Direction      | 0.6 MB    | ~50   | Lightweight MobileNet          |

### Graph Optimizations

The `OnnxInterpreter.optimize_graph()` pass applies before execution:

1. **BatchNorm folding** -- folds BN scale/bias/mean/var into preceding Conv weights (eliminates BN entirely)
2. **Identity elimination** -- removes Identity nodes, rewires edges
3. **Conv+Activation fusion** -- fuses Conv followed by Relu, HardSigmoid, or HardSwish into a single FusedConvActivation node (eliminates 62 kernel launches in the detector)

## Performance

| Metric              | ONNX Runtime (CPU) | molt/tinygrad (WASM) | molt/tinygrad (WebGPU) |
|---------------------|--------------------|----------------------|------------------------|
| Detection           | 4.8 ms             | TBD                  | TBD                    |
| Recognition         | 4.2 ms             | TBD                  | TBD                    |
| Binary size         | 16.1 MB            | 10.3 MB (3.2 gzip)  | 10.3 MB (3.2 gzip)    |
| WASM compile        | N/A                | 7.6 ms               | N/A                    |
| WASM instantiate    | N/A                | 2.0 ms               | N/A                    |
| WASM total startup  | N/A                | 9.6 ms               | N/A                    |
| Conv2d JS (64ch)    | N/A                | 5.6 ms (2.5 GFLOPS) | TBD                    |

WASM startup measured via Node.js WebAssembly.compile + instantiate (M3 Pro, cold start).
End-to-end inference benchmarks require weight loading into the WASM module (harness TBD).

### WASM Exports

The compiled module exports the full PaddleOCR inference pipeline:
- `tinygrad_paddleocr_driver__init` / `tinygrad_paddleocr_driver__init_full`
- `tinygrad_paddleocr_driver__detect_only`
- `tinygrad_paddleocr_driver__ocr`
- `tinygrad_paddleocr_driver___rgb_bytes_to_tensor`
- molt runtime functions: `molt_alloc`, `molt_main`, `molt_isolate_bootstrap`, etc.

## Supported Languages

| Language    | Dictionary File        | Characters |
|-------------|------------------------|------------|
| English     | en_dict.txt            | 95         |
| Chinese     | ppocr_keys_v1.txt      | 6,623      |
| Japanese    | japan_dict.txt         | 4,399      |
| Korean      | korean_dict.txt        | 3,679      |
| Latin       | latin_dict.txt         | 130        |
| Cyrillic    | cyrillic_dict.txt      | 116        |
| Devanagari  | devanagari_dict.txt    | 143        |
| Arabic      | arabic_dict.txt        | 117        |
| Chinese Trad| chinese_cht_dict.txt   | 8,414      |
| PPOCRv5     | ppocrv5_dict.txt       | 18,207     |
| English+    | en_ppocr_dict.txt      | 388        |

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

## Model Provenance

- Detector: [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) ch_PP-OCRv4_det
- Recognizer: [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) ch_PP-OCRv4_rec
- ONNX conversion: [OleehyO/paddleocrv4.onnx](https://huggingface.co/OleehyO/paddleocrv4.onnx)
- Dictionaries: PaddleOCR official ppocr/utils/dict/

## License

Apache 2.0 (same as PaddleOCR upstream).
