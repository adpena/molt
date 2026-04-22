# Nemotron OCR v2: Strategic Assessment

**Date**: 2026-04-14
**Status**: Research complete. Nemotron v2 is a fundamentally different architecture from Falcon-OCR. Not a drop-in replacement, but a compelling complement for specific deployment targets.

## Architecture Comparison

| Property | Falcon-OCR (300M VLM) | Nemotron OCR v2 English | Nemotron OCR v2 Multilingual |
|----------|----------------------|------------------------|------------------------------|
| **Architecture** | Vision-Language Model (autoregressive) | 3-stage pipeline: detector + recognizer + relational | Same |
| **Detector** | Integrated vision encoder | RegNetX-8GF (45.4M params) | RegNetX-8GF (45.4M params) |
| **Recognizer** | Causal LM decoder | Pre-norm Transformer (6.1M params, 3 layers) | Pre-norm Transformer (36.1M params, 6 layers) |
| **Relational** | N/A (token sequence) | Multi-layer relational (2.3M params) | Multi-layer relational (2.3M params) |
| **Total params** | ~300M | **53.8M** | **83.9M** |
| **Approach** | Generate text token-by-token | Detect regions, recognize per-crop, reorder | Same |

### Key Insight: Different Paradigms

Falcon-OCR is a **generative VLM** — it sees the whole image and produces text autoregressively. Nemotron v2 is a **classical OCR pipeline** — detect bounding boxes, crop regions, recognize text in each crop, then use a relational model to determine reading order.

This has profound implications for deployment:

- **Falcon-OCR**: Quality scales with decode steps. Slow but handles arbitrary layouts.
- **Nemotron v2**: Quality scales with detector recall. Fast because recognition is parallel per-crop.

## Size and Weight Analysis

### Nemotron v2 English Weights

| File | Size | Component |
|------|------|-----------|
| `detector.pth` | 182.0 MB | RegNetX-8GF backbone (F32) |
| `recognizer.pth` | 24.6 MB | Transformer recognizer (F32) |
| `relational.pth` | 9.0 MB | Layout relational model (F32) |
| **Total** | **215.6 MB** | **53.8M params at F32** |

### Quantized Size Estimates

| Precision | Detector | Recognizer | Relational | Total |
|-----------|----------|------------|------------|-------|
| F32 | 182 MB | 24.6 MB | 9.0 MB | 215.6 MB |
| F16 | 91 MB | 12.3 MB | 4.5 MB | 107.8 MB |
| INT8 | 45.5 MB | 6.2 MB | 2.3 MB | **54.0 MB** |
| INT4 | 22.8 MB | 3.1 MB | 1.1 MB | 27.0 MB |

An ONNX INT8 quantized detector already exists at `satyadevineni/Nemotron_OCR_V2_Detector_EN` (45.6 MB).

## Speed Comparison (OmniDocBench, A100)

| Model | pages/sec | English NED (lower=better) |
|-------|----------|---------------------------|
| PaddleOCR v5 (server) | 1.2 | 0.027 |
| OpenOCR (server) | 1.5 | 0.024 |
| EasyOCR | 0.4 | 0.095 |
| **Nemotron OCR v2 (EN)** | **40.7** | **0.038** |
| **Nemotron OCR v2 (multi)** | **34.7** | **0.048** |

Nemotron v2 English is **34x faster** than PaddleOCR v5 with only marginally higher error (0.038 vs 0.027 NED). For invoice OCR where English dominance is expected, this is an excellent tradeoff.

## Deployment Feasibility

### Cloudflare Workers (128 MB memory limit)

**Verdict: Possible for English variant at INT8, but requires ONNX/WASM runtime.**

- INT8 English total: ~54 MB. Fits within 128 MB Worker memory.
- However, Nemotron v2 uses PyTorch with a C++ CUDA extension. No WASM build exists.
- The ONNX detector export exists (45.6 MB INT8), but recognizer and relational ONNX exports do not yet exist.
- Would need: ONNX exports for all 3 components + onnxruntime-web WASM backend.
- The RegNetX-8GF detector is a standard ConvNet — ONNX export is straightforward.
- The Transformer recognizer is also standard — ONNX export is feasible.
- The relational model uses custom ops — needs investigation.

### Browser WebGPU/WASM

**Verdict: Feasible but requires significant engineering.**

- No GGUF version exists (not an LLM, so GGUF/llama.cpp is not applicable).
- CoreML export exists (`mweinbach/nemotron-ocr-v2-coreml`) — confirms the architecture is exportable.
- ONNX -> WebGPU via onnxruntime-web is the viable path.
- Would need custom pre/post-processing in JS (NMS, crop extraction, reading order).
- Total download: ~54 MB INT8 (vs ~260 MB for Falcon-OCR INT8) — significantly smaller.

### Modal / GPU Cloud

**Verdict: Trivial. The official Docker image works out of the box.**

- `docker compose run --rm nemotron-ocr` with GPU access.
- 34-40 pages/sec on A100, likely 15-20 pages/sec on A10G.
- Perfect for batch processing and API agents.
- Could run alongside Falcon-OCR on same Modal deployment.

## Quality Assessment for Invoice OCR

### Strengths
- Bounding box output with coordinates — essential for structured invoice extraction.
- Confidence scores per text region — enables quality-gated pipelines.
- Reading order via relational model — handles multi-column invoices.
- 34x faster than PaddleOCR — enables real-time batch processing.

### Weaknesses
- English NED 0.038 vs PaddleOCR 0.027 — slightly less accurate on benchmarks.
- No GGUF/llama.cpp path — the "GGUF OCR" discovery does not apply here.
- Requires NVIDIA GPU (CUDA C++ extension) — no CPU-only inference without ONNX export.
- The relational model adds complexity for structured extraction.

### For Our Invoice Use Case
- Invoices are primarily English, structured layouts with tables.
- Bounding boxes are more useful than raw text for field extraction.
- Speed matters less in browser (single image) but hugely in batch API.
- The 0.038 NED on English is well within acceptable range for invoices.

## Migration Path

### Phase 1: Modal GPU Deployment (immediate)
- Add Nemotron v2 as a second Modal endpoint alongside Falcon-OCR.
- Use for batch processing where speed matters (34 pages/sec vs Falcon's 2.9 img/s).
- Falcon-OCR remains primary for single-image browser inference.

### Phase 2: ONNX Export Pipeline (1-2 weeks)
- Export recognizer and relational models to ONNX.
- Validate INT8 quantized accuracy on invoice test set.
- Build ONNX inference pipeline (Python, no CUDA dependency).

### Phase 3: Browser WASM Deployment (2-4 weeks)
- Port ONNX models to onnxruntime-web.
- Implement JS pre/post-processing (NMS, crop, reading order).
- Total browser download drops from ~260 MB to ~54 MB.
- Faster inference (parallel crop recognition vs autoregressive decode).

### Phase 4: Workers Edge Deployment (4-6 weeks)
- If ONNX runtime fits in Workers memory with INT8 models.
- Would enable server-side OCR without GPU — major cost reduction.
- Fallback: keep Workers as weight-serving CDN, run inference in browser.

## Strategic Recommendation

**Do not replace Falcon-OCR with Nemotron v2. Use both.**

| Deployment | Model | Reason |
|------------|-------|--------|
| Browser (single image) | Falcon-OCR INT8 | Already deployed, working, VLM handles arbitrary layouts |
| API batch processing | Nemotron v2 EN | 34x faster, bounding box output, structured extraction |
| Workers edge | Falcon-OCR WASM | Already deployed, too much engineering to port Nemotron |
| Future browser | Nemotron v2 ONNX | Smaller download (54 MB vs 260 MB), parallel recognition |

The llama.cpp GGUF discovery is **not applicable** to Nemotron v2 — it is not an autoregressive LLM. GGUF quantization targets transformer decoders. Nemotron's detector is a ConvNet (RegNetX), and its recognizer is a small encoder-only transformer. The correct quantization path is ONNX INT8/INT4, which already partially exists.

## Open Questions

1. Can the relational model be exported to ONNX without custom ops?
2. What is the actual INT8 accuracy degradation on invoice images?
3. Does onnxruntime-web support RegNetX convolutions efficiently?
4. Can we run the 3-stage pipeline with acceptable latency in browser WASM?
5. Is there a community ONNX export for the full pipeline (not just detector)?
