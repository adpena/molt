# World's First: Compiled Tinygrad to WASM with WebGPU Inference

## What we built
The first-ever AOT compilation of tinygrad tensor operations to WebAssembly,
with browser-side GPU inference via WebGPU compute shaders.

## Why this is unprecedented
1. tinygrad has never been AOT-compiled (always Python runtime)
2. tinygrad has never run in a browser
3. tinygrad has never generated WebGPU compute shaders for browser inference
4. No one has compiled a tinygrad-based model to WASM and deployed it to production

## The stack
- 26 tinygrad-conformant primitives (Rust, molt-gpu crate)
- 7 shader renderers (MSL, WGSL, GLSL, CUDA, HIP, OpenCL, MIL)
- AOT Python -> WASM compilation (molt compiler)
- WebGPU compute dispatch in browser
- Production deployment on Cloudflare Workers

## Production use case
Falcon-OCR (300M param) document OCR running in the browser via
compiled tinygrad tensor operations dispatched to the user's GPU.

## Comparison
| Framework | AOT Compiled | Browser | WebGPU | Production |
|-----------|-------------|---------|--------|-----------|
| PyTorch | No | No | No | Server only |
| TensorFlow.js | No (JIT) | Yes | Yes | Yes |
| ONNX Runtime Web | No (runtime) | Yes | Yes | Yes |
| tinygrad | No | No | No | openpilot only |
| **molt + tinygrad** | **Yes** | **Yes** | **Yes** | **Yes** |
