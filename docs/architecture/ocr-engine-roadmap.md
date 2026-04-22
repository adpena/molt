# OCR Engine Roadmap

## Phase 1: Foundation (DONE)
- [x] 26 tinygrad primitives in Rust (molt-gpu crate)
- [x] 7 shader renderers (MSL, WGSL, GLSL, CUDA, HIP, OpenCL, MIL)
- [x] WASM compilation pipeline (molt compiler)
- [x] Browser WebGPU compute engine
- [x] Cloudflare Worker deployment

## Phase 2: PaddleOCR via molt (IN PROGRESS)
- [x] Download PaddleOCR ONNX models (16 MB total)
- [x] Upload to R2
- [x] Create tinygrad inference scaffold (paddleocr.py)
- [x] Implement ONNX weight loading (OnnxWeightParser + WeightStore)
  - Handles Constant graph nodes (PaddleOCR format, not graph.initializer)
  - Supports float32, int32, int64 dtypes
  - onnx library fast path with raw protobuf fallback
  - Cross-validated: 342 det / 406 rec / 308 cls constants extracted
- [x] Worker endpoint /ocr/paddle-molt
- [x] Compile to WASM via molt
- [x] Harden ONNX interpreter op surface used by PaddleOCR
  - Covers detector/recognizer/classifier op families including Conv,
    ConvTranspose, MaxPool, AveragePool, Slice, Resize, MatMul, Softmax,
    Squeeze, Unsqueeze, and shape/cast ops.
  - Fail-fast validation is in place for unsupported Resize modes, invalid
    Slice inputs/steps, invalid Cast targets, invalid Squeeze/Unsqueeze axes,
    and invalid grouped Conv channel contracts.
- [x] Deterministic local PaddleOCR artifact discovery for tests
  - `MOLT_PADDLEOCR_MODEL_ROOT` is the canonical override.
  - Tests use explicit relative candidates and `pytest.skip`, not recursive
    cache globs or print-and-return skips.
- [ ] Prove DBNet detector raw forward parity against ONNX Runtime
- [ ] Prove SVTR recognizer raw forward parity against ONNX Runtime
- [ ] Test on real invoices
- [ ] Benchmark: compiled vs onnxruntime
- [ ] Wire real model artifacts into end-to-end WASM execution
- [ ] Resolve English recognizer character mapping / blank-token alignment

## Phase 3: Falcon-OCR Heavy Duty (DONE)
- [x] WASM binary (2.9 MB gzipped)
- [x] WebGPU compute kernels
- [x] Speculative decoding
- [x] INT4/INT8 quantization
- [ ] Production quality validation

## Phase 4: Nemotron v2 (FUTURE)
- [ ] ONNX export of 3-stage pipeline
- [ ] Integrate as replacement for PaddleOCR speed
- [ ] 28x throughput improvement

## Phase 5: Model Cards (FUTURE)
- [ ] Publish molt-paddleocr on HuggingFace
- [ ] Publish molt-falcon-ocr on HuggingFace
