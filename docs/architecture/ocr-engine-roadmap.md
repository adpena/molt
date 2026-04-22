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
- [ ] Implement DBNet detector forward pass
- [ ] Implement SVTR recognizer forward pass
- [ ] Test on real invoices
- [ ] Compile to WASM via molt
- [ ] Benchmark: compiled vs onnxruntime

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
