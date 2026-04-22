# ONNX Baseline for Falcon-OCR

## Why ONNX as baseline
- Mature runtime (onnxruntime-web has WebGPU backend since 2024)
- Proven browser deployment (used by Transformers.js)
- Known performance characteristics
- Quality benchmark: ONNX output = reference output

## Deployment path
1. Export Falcon-OCR to ONNX (export-onnx.py in enjoice)
2. Quantize to INT8/FP16 (quantize.py in enjoice)
3. Load in browser via onnxruntime-web
4. Compare speed and quality against molt-compiled tinygrad

## What molt must beat
- ONNX Runtime Web WebGPU: estimated 5-20 tok/s on M1
- ONNX INT8 model size: ~300 MB (or ~150 MB FP16)
- Quality: same as PyTorch reference (ONNX is lossless export)

## How molt can beat ONNX
- Kernel fusion (ONNX Runtime does some, molt does aggressive fusion)
- Custom WebGPU kernels (our WGSL shaders are hand-optimized)
- Speculative decoding (ONNX Runtime doesn't support this)
- Tiered KV cache (ONNX Runtime has basic KV cache)
- Tree-shaked binary (ONNX includes all ops, molt only includes used ops)
