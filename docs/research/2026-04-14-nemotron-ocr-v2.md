# Nemotron OCR v2: Strategic Assessment

**Date**: 2026-04-14

**Updated**: 2026-04-22

**Status**: Research memo plus first wrapper hardening. This is not a claim that
Molt has a native Nemotron runtime, browser runtime, Workers runtime, ONNX export,
or llama.cpp/GGUF execution path.

## Evidence Classes

Use these labels when extending this work:

- **Upstream fact**: stated by NVIDIA, llama.cpp, arXiv, or another cited source.
- **Repo-proven**: backed by committed Molt tests or measured artifacts.
- **Hypothesis**: technically plausible but not yet implemented or measured in
  this repo.

Current repo-proven scope:

- `deploy/modal/nemotron_ocr.py` validates `lang` and `merge_level`, cleans temp
  image files, formats structured OCR output, and calls `NemotronOCRV2` once for
  batch input. Boundary coverage lives in
  `tests/test_nemotron_ocr_boundaries.py`.
- `src/molt/gpu/gguf.py` is a parser/dequantizer only. It does not execute OCR
  models, tokenizers, image preprocessors, multimodal projectors, or llama.cpp
  graphs.

## Architecture Comparison

| Property | Falcon-OCR | Nemotron OCR v2 English | Nemotron OCR v2 Multilingual |
|---|---|---|---|
| Architecture | Generative VLM | Detector + recognizer + relational layout | Same |
| Detector | Integrated vision encoder | RegNetX-8GF, 45.4M params | RegNetX-8GF, 45.4M params |
| Recognizer | Causal LM decoder | Pre-norm Transformer, 6.1M params, 3 layers | Pre-norm Transformer, 36.1M params, 6 layers |
| Relational model | N/A | 2.3M params | 2.3M params |
| Total params | Approximately 300M in current planning docs | 53.8M | 83.9M |
| Output shape | Prompted token text | Bboxes, text, confidence, reading-order grouping | Same |

Nemotron v2 and Falcon-OCR are different model families. Falcon-OCR quality and
latency are dominated by autoregressive generation. Nemotron v2 quality depends
on detector recall, recognition quality, and relational ordering. This makes
Nemotron a strong candidate for structured OCR and batch invoice extraction, but
it is not a drop-in replacement for a promptable VLM.

## Upstream Facts

NVIDIA's model card states that Nemotron OCR v2:

- ships English and multilingual variants;
- uses a RegNetX-8GF detector, pre-norm Transformer recognizer, and relational
  layout model;
- has 53,831,335 parameters for English and 83,853,044 for multilingual;
- accepts RGB image input and returns structured OCR regions;
- is currently integrated through PyTorch on Linux with NVIDIA GPUs and a C++
  CUDA extension.

NVIDIA's 2026 Hugging Face article reports A100 benchmark throughput of
40.7 pages/sec for English and 34.7 pages/sec for multilingual on OmniDocBench.
Those numbers are upstream benchmark claims until reproduced by Molt benchmarks.

NVIDIA's `pipeline_v2.py` source documents the important batch contract:
`NemotronOCRV2.__call__` accepts a single image or a list of images, returns a
flat `list[dict]` for single input, and returns `list[list[dict]]` for batch
input. The Molt Modal wrapper now treats that as the only supported batch path;
it does not silently serial-loop and call that native batching.

## Size And Quantization Hypotheses

The English checkpoint has roughly 53.8M parameters. A naive storage estimate is:

| Precision | Approximate size |
|---|---:|
| F32 | 215 MB |
| F16 | 108 MB |
| INT8 | 54 MB |
| INT4 | 27 MB |

These are storage estimates, not deployment proof. A browser or Workers runtime
must also account for runtime code, activation memory, temporary tensors,
pre/post-processing buffers, and allocator overhead. The 54 MB INT8 path is a
useful target, not a guarantee.

## Deployment Assessment

### Modal / GPU Cloud

Status: partially implemented wrapper, not production-proven.

The current Modal wrapper is a clean smokeable boundary around the upstream
PyTorch/CUDA package. It still needs real GPU deployment proof, load tests,
latency histograms, and cost/performance baselines before any production claim.
Throughput must be measured on the actual Modal GPU profile rather than inferred
from A100 numbers.

### Browser WebGPU/WASM

Status: hypothesis.

The likely clean path is to re-derive the model in a Molt-owned model package,
then lower through tinygrad/libmolt/Molt GPU primitives. ONNX/WebGPU may be a
useful comparison path, but it must not become a hidden runtime dependency for
compiled Molt binaries. Work needed:

- RegNetX grouped/depthwise convolution coverage and parity tests;
- detector heads, NMS, rotated boxes, and crop extraction;
- Transformer recognizer with exact charset handling;
- relational layout model and reading-order parity;
- INT8/INT4 quantization with accuracy and activation-memory measurements;
- browser and Workers memory proof.

### Cloudflare Workers

Status: hypothesis.

Workers deployment is not proven by parameter count alone. A Worker path needs
explicit bundle size, runtime memory, cold-start, and timeout evidence. Until
then, Workers should be treated as a weight-serving/CDN/control-plane surface,
not as a proven Nemotron inference host.

## GGUF And llama.cpp Boundary

No known Nemotron OCR v2 GGUF artifact is part of the current plan, and Nemotron
is not naturally a llama.cpp decoder-only OCR model. GGUF still matters for a
separate OCR baseline lane because llama.cpp now supports OCR-oriented
multimodal models such as GLM-OCR, LightOnOCR, Qianfan-OCR, PaddleOCR-VL,
DeepSeek-OCR, dots.ocr, and HunyuanOCR.

Boundary rule:

- `molt.gpu.gguf` may parse GGUF files and expose tensors/metadata.
- OCR runtime support requires an explicit model executor, image preprocessing
  contract, tokenizer/projector contract, decoding contract, tests, and
  provenance. A GGUF parser alone is not OCR support.
- llama.cpp can be used as an external comparison/oracle or as a libmolt
  extension experiment, but it must not bypass Molt's runtime ownership.

## Recommended Tranche

1. Keep Nemotron model code and weights outside Molt core, ideally in a
   `molt-ocr` or `molt-falcon-ocr` style package with explicit artifact
   manifests and provenance.
2. Use Nemotron as a tinygrad/Molt GPU hardening target: RegNetX, batched image
   preprocessing, NMS, crop/ROI transforms, Transformer recognizer, and
   relational layout all exercise different missing pieces.
3. Use llama.cpp OCR/GGUF models as external accuracy/performance baselines, not
   as a substitute for native Molt lowering.
4. Build a canonical OCR benchmark corpus with invoices, receipts, dense
   documents, tables, rotated scans, low-quality photos, and multilingual pages.
   Track NED/CER, bbox IoU, reading order, field extraction F1, P50/P95 latency,
   peak memory, download size, and cold start.

## Provenance

- NVIDIA, "Nemotron OCR v2" model card. Model architecture, parameter counts,
  current PyTorch/CUDA integration, license, output structure, and upstream
  benchmark tables. https://huggingface.co/nvidia/nemotron-ocr-v2
- NVIDIA, "Building a Fast Multilingual OCR Model with Synthetic Data", 2026.
  FOTS-style shared-backbone rationale and reported A100 throughput.
  https://huggingface.co/blog/nvidia/nemotron-ocr-v2
- NVIDIA `NemotronOCRV2` pipeline source. Batch API shape and return contract.
  https://huggingface.co/nvidia/nemotron-ocr-v2/blame/f7389050a37cf705e47ba26762ba0bb34f901bff/nemotron-ocr/src/nemotron_ocr/inference/pipeline_v2.py
- Liu, Liang, Yan, Chen, Qiao, Yan, "FOTS: Fast Oriented Text Spotting with a
  Unified Network", arXiv:1801.01671, 2018. https://arxiv.org/abs/1801.01671
- Radosavovic, Kosaraju, Girshick, He, Dollar, "Designing Network Design
  Spaces", arXiv:2003.13678, 2020. RegNet provenance.
  https://arxiv.org/abs/2003.13678
- Ouyang et al., "OmniDocBench: Benchmarking Diverse PDF Document Parsing with
  Comprehensive Annotations", arXiv:2412.07626, CVPR 2025.
  https://arxiv.org/abs/2412.07626
- ggml-org, "Using OCR models with llama.cpp", 2026. OCR model list, prompt
  examples, Q8_0/F16 guidance, and server/CLI usage.
  https://huggingface.co/blog/ggml-org/using-ocr-models-with-llama-cpp
- ggml-org/llama.cpp PR #19677, "model: support GLM-OCR", merged 2026-02-18.
  https://github.com/ggml-org/llama.cpp/pull/19677
- Duan et al., "GLM-OCR Technical Report", arXiv:2603.10910, 2026.
  https://arxiv.org/abs/2603.10910
- Taghadouini, Cavailles, Aubertin, "LightOnOCR: A 1B End-to-End Multilingual
  Vision-Language Model for State-of-the-Art OCR", arXiv:2601.14251, 2026.
  https://arxiv.org/abs/2601.14251
- Dong et al., "Qianfan-OCR: A Unified End-to-End Model for Document
  Intelligence", arXiv:2603.13398, 2026. https://arxiv.org/abs/2603.13398
- Hunyuan Vision Team et al., "HunyuanOCR Technical Report", arXiv:2511.19575,
  2025. https://arxiv.org/abs/2511.19575
- Li, Yang, Liu, Wang, Zhang, "dots.ocr: Multilingual Document Layout Parsing in
  a Single Vision-Language Model", arXiv:2512.02498, 2025.
  https://arxiv.org/abs/2512.02498
