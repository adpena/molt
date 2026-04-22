# ONNX Baseline Plan for Falcon-OCR

**Date:** 2026-04-21

This is a baseline plan, not benchmark evidence. The goal is to compare a
Molt/tinygrad browser path against a conventional ONNX Runtime Web path with the
same model, prompts, tokenizer, image preprocessing, and evaluation corpus.

## Why ONNX Is A Useful Baseline

- ONNX Runtime Web has mature browser deployment paths, including WebGPU support.
- ONNX gives a common graph interchange target for comparing model conversion,
  quantization, startup, memory, and inference latency.
- A successful ONNX export can act as a reference lane only after parity against
  the source model has been measured.

## Required Baseline Work

1. Export Falcon-OCR to ONNX with a documented script and pinned dependencies.
2. Validate numerics against the source model on fixed image/prompt fixtures.
3. Quantize with documented INT8/FP16 settings and measure quality deltas.
4. Run ONNX Runtime Web in browser with dated browser/hardware versions.
5. Compare Molt against ONNX on the same corpus and metrics.

## Metrics

- Model artifact size and compressed transfer size.
- Cold start, warm start, and first-token latency.
- Tokens/sec or pages/sec, depending on the benchmark task.
- Peak browser memory and GPU memory where observable.
- OCR quality metrics: CER/WER/NED, table extraction accuracy, and structured
  invoice field F1.

## Non-Claims

Do not claim that Molt beats ONNX, that ONNX is lossless, or that a specific
throughput is expected until this file links to measured artifacts. Any future
performance table should include command lines, commit SHA, browser version,
hardware, and input corpus details.
