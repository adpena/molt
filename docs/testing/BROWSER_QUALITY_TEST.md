# Browser OCR Quality Test Guide

## Infrastructure Status

All endpoints verified serving (2026-04-14):

| Asset | URL | Status |
|-------|-----|--------|
| WASM engine | `/wasm/falcon-ocr.wasm` | 200 (10.1 MB) |
| WebGPU runtime | `/browser/webgpu-engine.js` | 200 (53 KB) |
| INT8 shard 1 | `/weights/falcon-ocr-int8/model-00001-of-00006.safetensors` | 200 (46.7 MB) |
| Tokenizer | `/weights/falcon-ocr/tokenizer.json` | 200 (4.8 MB) |
| Test page | `/test` | 200 |
| Dashboard | `/dashboard` | 200 |

## Quick Test

1. Open <https://falcon-ocr.adpena.workers.dev/test> in Chrome (M1/M2 MacBook recommended)
2. Check GPU detection status (should show "WebGPU" or "WebGL2")
3. Wait for model download (~260 MB for INT8, cached after first load)
4. Upload an invoice image (PNG or JPEG)
5. Observe:
   - GPU backend detected
   - Inference time per token
   - Extracted text quality
   - Total token count

## Expected Performance

| Backend | Tokens/sec | Notes |
|---------|-----------|-------|
| WebGPU (Metal) | 0.1-1.0 tok/s | GPU matmul via compute shaders |
| WebGL2 | 0.03-0.3 tok/s | Fragment shader fallback |
| WASM SIMD | 0.015-0.03 tok/s | CPU-only, last resort |

INT8 quantization produces readable OCR text. INT4 is degraded (14% quantization error).

## What to Evaluate

### Correctness
- Does the model detect all text regions in the image?
- Are numbers and monetary amounts extracted correctly?
- Is the vendor/company name found?
- Are line items parsed with correct quantities and prices?

### Quality Comparison
- Compare extracted text against PaddleOCR output for same image
- Note any systematic errors (character confusion, missing decimals)
- Check multi-line text ordering

### Performance
- Record first-token latency (cold model load vs cached)
- Record total inference time for full page
- Note GPU utilization if visible in dashboard

## Test Images

Use real invoice images from the `deploy/enjoice/test-fixtures/` directory if available, or any invoice PDF/PNG. Priority test cases:

1. Clean typed invoice (baseline)
2. Scanned/photographed invoice (noise resilience)
3. Multi-column invoice with tables (layout handling)
4. Low-contrast or faded text (edge case)
