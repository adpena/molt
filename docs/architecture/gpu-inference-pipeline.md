# Falcon-OCR GPU Inference Pipeline

## Browser (WebGPU)

1. Image -> PNG decode -> resize to 128x128 -> 16x16 patches -> normalize [-1,1]
2. Patches -> embedding lookup (CPU, small table)
3. Prepend `<|OCR_PLAIN|>` token (ID 257)
4. For each transformer layer (22 total):
   a. RMSNorm (GPU: fused reduce + scale)
   b. QKV projection (GPU: batched matmul, 3 dispatches in 1 command buffer)
   c. RoPE rotation (GPU: fused sin/cos)
   d. Attention: Q*K^T -> scale -> mask -> softmax -> V (GPU: 3 matmuls + softmax)
   e. Output projection (GPU: matmul)
   f. Residual add (GPU: elementwise)
   g. FFN: RMSNorm -> gate/up -> SiLU -> mul -> down (GPU: 3 matmuls + elementwise)
   h. Residual add (GPU: elementwise)
5. Final RMSNorm (GPU)
6. Output projection -> logits (GPU: matmul)
7. Argmax -> token ID (CPU)
8. Repeat from step 4 for next token (autoregressive)
9. Decode tokens -> text via tokenizer

## Compute backends (priority order)

- **WebGPU WGSL compute shaders** (10-100x faster than CPU)
- **WebGL2 GLSL fragment shaders** (3-30x faster than CPU)
- **WASM SIMD f32x4** (2-4x faster than scalar)
- **Scalar JS** (baseline, last resort)

## GPU dispatch optimization

### Batched QKV projections

Instead of 3 separate `device.queue.submit()` calls for Q, K, V projections,
all three matmuls are recorded in a single compute pass and submitted once:

```javascript
// Single command encoder, single submit for all 3 projections.
const encoder = device.createCommandEncoder();
const pass = encoder.beginComputePass();

pass.setPipeline(matmulPipeline);
pass.setBindGroup(0, qBindGroup);
pass.dispatchWorkgroups(qWorkgroups);

pass.setBindGroup(0, kBindGroup);
pass.dispatchWorkgroups(kvWorkgroups);

pass.setBindGroup(0, vBindGroup);
pass.dispatchWorkgroups(kvWorkgroups);

pass.end();
device.queue.submit([encoder.finish()]);  // ONE submit instead of THREE
```

This eliminates ~2ms of CPU-GPU sync overhead per layer (66% dispatch reduction
for the QKV step). Over 22 layers, this saves ~44ms per token.

Pre-concatenation of Q/K/V weights into a single matrix is not practical for
GQA (grouped-query attention) where Q has more heads than K/V, resulting in
different output dimensions. The batched approach handles heterogeneous dimensions.

### Minimizing CPU-GPU sync

All intermediate tensors stay on GPU as `GPUBuffer` objects. The `*GPU()` method
variants (matmulGPU, softmaxGPU, rmsNormGPU, ropeGPU, addGPU, mulGPU) return
GPUBuffers without any readback. Only the final logits are read back to CPU for
argmax token selection.

```
Layer input (GPUBuffer)
  -> rmsNormGPU -> matmulBatchQKV -> ropeGPU -> matmulGPU (attention)
  -> softmaxGPU -> matmulGPU -> matmulGPU (out proj) -> addGPU (residual)
  -> rmsNormGPU -> matmulGPU (FFN) -> mulGPU -> matmulGPU -> addGPU
  -> Layer output (GPUBuffer)

Only at the very end:
  -> Final logits matmulGPU -> readBuffer() -> CPU argmax
```

### Command encoder pipelining

The `forwardLayerGPU()` method records ALL operations for one transformer layer
and submits them together. The GPU driver can then schedule work optimally,
overlapping memory transfers with compute where hardware supports it.

## Quantization and quality

### INT4 (current, garbage quality on 300M model)

- 4 bits per weight, 16 discrete levels
- Dequantization: `float_weight = scale * int4_value` (f32)
- Matmul: `output = input @ dequantized_weights` (f32 accumulation)
- Quality: CER > 40% — insufficient precision for 300M parameters

### INT8 (target, coherent quality)

- 8 bits per weight, 256 discrete levels
- ~16x less quantization error than INT4
- Quality: CER < 8% — sufficient for OCR tasks
- Model size: ~300MB (vs ~150MB for INT4, ~1.2GB for FP32)

### Why GPU f32 matmul does NOT fix INT4 quality

Both CPU and GPU perform identical f32 arithmetic after dequantization. The GPU's
`fma()` instruction has marginally better precision (no intermediate rounding on
multiply-add), but this difference is at the ULP level — far below the massive
quantization noise from INT4. The quality bottleneck is the 4-bit weight
representation, not the compute precision.

## Speculative decoding (browser only)

- **Draft model**: first 4 layers, WebGPU (fast, low quality predictions)
- **Target model**: all 22 layers, WebGPU (slower, full quality)
- **Accept/reject**: compare draft vs target tokens
- **Expected speedup**: 3-5x more tokens per GPU batch cycle

The draft model runs the first 4 transformer layers to generate candidate tokens
quickly. The full 22-layer target model then verifies these candidates in a single
batched forward pass. Accepted tokens are emitted immediately; rejected tokens
trigger re-generation from the target model.

This exploits the observation that most tokens (especially common words and
punctuation in OCR output) are predictable from shallow context, so the draft
model's acceptance rate is high for typical invoice/document OCR.

## Memory budget (browser)

| Component | Size | Notes |
|-----------|------|-------|
| INT8 weights | ~300 MB | 22 layers, all projections |
| KV cache | ~12 MB | seqLen=256, 22 layers |
| Activations | ~6 MB | Double-buffered per layer |
| Buffer pool | ~20 MB | Reusable GPU buffers |
| **Total GPU memory** | **~340 MB** | Well within 4GB WebGPU limit |

## ONNX Interpreter -> WebGPU Dispatch

When running in the browser with WebGPU available:
1. ONNX interpreter encounters a Conv node
2. Instead of running conv2d on CPU, it calls the WebGPU engine's conv2d()
3. The GPU kernel processes all 62 Conv nodes in parallel
4. Results stay on GPU (no readback until final output)
5. Expected speedup: 10-50x for the Conv-heavy detection path

The dispatch is via the compute engine abstraction:
- Python (WASM) -> JS bridge -> ComputeEngine.conv2d() -> WebGPU dispatch

### PaddleOCR Conv dispatch path

The PaddleOCR detector (DBNet PP-OCRv4) has 62 Conv layers totaling ~60% of
inference compute. When the ONNX interpreter walks these nodes:

```
OnnxInterpreter.run()
  -> node.op_type == "Conv"
  -> calls tinygrad Conv2d primitive
  -> molt WASM runtime -> JS host import bridge
  -> ComputeEngine.conv2dGPU(input, weight, bias, ...)
  -> WebGPU dispatch: TILE_SIZE=16, workgroups=(cOut/16, oh/16, ow/16)
  -> result stays as GPUBuffer (no readback)
```

The 7 WebGPU kernels available for PaddleOCR dispatch:
1. `molt_conv2d` -- direct convolution (62 layers)
2. `molt_kernel` -- general matmul (SVTRv2 attention)
3. `molt_softmax` -- attention softmax
4. `molt_rms_norm` -- layer normalization
5. `molt_rope` -- rotary position embeddings
6. `molt_add` -- residual connections
7. `molt_mul` -- elementwise multiply (gating)

Conv+Activation fusion (from OnnxInterpreter.optimize_graph()) means the
62 Conv+Relu/HardSigmoid/HardSwish pairs become 62 fused kernel launches
instead of 124 separate dispatches.

## Performance targets

| Metric | Target | Notes |
|--------|--------|-------|
| First token latency | < 200ms | WebGPU warm, weights cached |
| Tokens/second | > 30 tok/s | INT8, single image |
| Full invoice OCR | < 3s | ~80 tokens average |
| Cold start (no cache) | < 5s | Weight download + compile |
