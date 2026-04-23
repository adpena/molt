# Nemotron OCR v2 ONNX Export Research

**Date**: 2026-04-22
**Status**: Detector and Recognizer exported. Relational model blocked by C++ extension.

## Model Architecture

Nemotron OCR v2 (`nvidia/nemotron-ocr-v2`) is a 3-stage pipeline:

| Stage | Architecture | EN Params | Multi Params | F32 Size |
|-------|-------------|-----------|-------------|----------|
| Detector | RegNetX-8GF backbone + FPN merge + ASPP output head | ~45M | same (shared weights) | 182 MB |
| Recognizer | CNN encoder + Pre-norm Transformer (3/6 layers) | ~6M | ~36M | 25 / 145 MB |
| Relational | Transformer encoder (4 layers) + geometric encoding | ~2.3M | same | 9 MB |

**Total**: ~53M EN / ~83M multi params.

## ONNX Export Results

### Detector (EN)

- **Export method**: Legacy TorchScript exporter (`dynamo=False`, opset 17)
- **Input**: `[batch, 3, height, width]` (dynamic H/W)
- **Outputs**: confidence map, rotated bounding boxes (RBOX), feature maps
- **F32**: 181.2 MB
- **INT8**: 45.7 MB (25% of F32)
- **Latency**: ~1112 ms on M4 Max CPU (640x640 input, ORT)

### Recognizer (EN)

- **Export method**: Dynamo exporter (`dynamo=True`, opset 18) -- required for TransformerEncoder decomposition
- **Input**: `[1, 128, 8, 32]` (fixed batch=1, rectified text crop)
- **Outputs**: logits `[1, 32, 858]`, features `[1, 32, 256]`
- **F32**: 24.7 MB
- **INT8**: 6.4 MB (26% of F32)
- **Latency**: ~3.9 ms on M4 Max CPU (single crop, ORT)

### Recognizer (Multilingual)

- **Same architecture**, 6 Transformer layers, 14247 tokens, 128-width sequence
- **F32**: 144.9 MB
- **INT8**: 36.9 MB (25% of F32)

### Relational Model

- **NOT exported**: depends on `nemotron_ocr_cpp` C++ extension for `quad_rectify_calc_quad_width` and `ragged_quad_all_2_all_distance_v2`
- These are geometric preprocessing ops (quad distance computation, reading order)
- The transformer encoder inside (4 layers, d=256) is exportable in isolation
- **Size**: 9 MB F32, would be ~2.5 MB INT8

## Key Technical Findings

1. **Legacy vs Dynamo exporter**: The legacy TorchScript exporter cannot export `aten::_transformer_encoder_layer_fwd` (fused transformer fast-path). The dynamo exporter decomposes it correctly but stores weights in external `.data` files that must be merged back with `onnx.save()`.

2. **Detector ONNX is clean**: Pure CNN + standard ops. Dynamic height/width axes work correctly. The ASPP shape comparison (`x.shape == out.shape`) generates a TracerWarning but traces correctly for fixed architecture.

3. **Recognizer batch dimension**: Dynamo export traces with fixed batch=1. For Workers deployment this is acceptable (process one crop at a time). For batched inference, re-export with explicit `dynamic_shapes`.

4. **RegNetX-8GF pretrained weights**: Auto-downloaded from PyTorch hub (~151 MB) during model construction. The detector `.pth` already includes the backbone weights, so the pretrained download is redundant at export time.

5. **INT4 quantization**: `onnxruntime.quantization.matmul_4bits_quantizer` is not available in the installed version. The detector is mostly conv layers (not MatMul), so INT4 via MatMul4Bits would have limited effect anyway. For true INT4, we would need `onnxruntime-extensions` or `onnxruntime-genai` with GPTQ/AWQ-style quantization.

## Size Summary (EN pipeline)

| Model | F32 | INT8 | Reduction |
|-------|-----|------|-----------|
| Detector | 181.2 MB | 45.7 MB | 75% |
| Recognizer | 24.7 MB | 6.4 MB | 74% |
| Relational | 9.0 MB | ~2.5 MB (est.) | ~72% |
| **Total** | **214.9 MB** | **54.6 MB** | **75%** |

## Workers Free Tier Feasibility

Workers free tier limits:
- 1 MB script size (compressed)
- 128 MB memory
- 10 ms CPU time per request

**Verdict: Does NOT fit Workers free tier.**

- The INT8 detector alone is 45.7 MB, far exceeding the 1 MB script limit
- Even with aggressive INT4 quantization (est. ~25 MB detector), it would still exceed limits
- The 10 ms CPU budget is insufficient -- detector alone takes ~1112 ms on M4 Max
- Workers AI (paid) with GPU inference is the viable deployment path

## Artifacts

Files in `/tmp/`:
- `nemotron_det_en.onnx` (181.2 MB) -- F32 detector
- `nemotron_det_en_int8.onnx` (45.7 MB) -- INT8 detector
- `nemotron_rec_en_full.onnx` (24.7 MB) -- F32 recognizer EN
- `nemotron_rec_en_int8.onnx` (6.4 MB) -- INT8 recognizer EN
- `nemotron_rec_ml_full.onnx` (144.9 MB) -- F32 recognizer multilingual
- `nemotron_rec_ml_int8.onnx` (36.9 MB) -- INT8 recognizer multilingual

## Next Steps

1. **R2 upload**: Upload INT8 models to R2 for persistent storage
2. **Workers AI**: Deploy via Workers AI with GPU inference (Cloudflare has ONNX Runtime support)
3. **Relational model**: Reimplement the C++ geometric ops in pure Python/ONNX custom ops, or use a simpler reading-order heuristic (top-to-bottom, left-to-right) for the Workers deployment
4. **Dynamic batch**: Re-export recognizer with `dynamic_shapes` for batch processing if needed
5. **INT4 exploration**: Use `optimum` or `auto-gptq` for proper weight-only INT4 quantization of the transformer layers
