---
title: molt-tinygrad OCR
emoji: "\U0001F50D"
colorFrom: blue
colorTo: green
sdk: static
pinned: false
license: apache-2.0
---

# molt/tinygrad OCR -- Compiled Inference in Your Browser

The first AOT-compiled tinygrad tensor operations running in the browser
via WebAssembly and WebGPU compute shaders.

## Demo
- **PaddleOCR** (99.6% accuracy, 10 languages): Upload an invoice, get text
- **Falcon-OCR** (300M VLM, complex docs): Heavy-duty document understanding
- **Whisper** (39M, speech-to-text): Coming soon

## Architecture
26 tinygrad primitives -> 7 shader renderers -> WebNN/WebGPU/WebGL2/WASM SIMD

## Try It
Visit [falcon-ocr.adpena.workers.dev/test](https://falcon-ocr.adpena.workers.dev/test)
