# Molt Tinygrad WASM/WebGPU Claim Ledger

This note tracks the current Molt + tinygrad browser-inference claim as an
internal evidence checklist. It is not a public priority claim and should not be
used as marketing copy without independent prior-art review and benchmark data.

## Current Working Claim

Molt is building an AOT Python/tinygrad path that can lower selected tensor
programs to WebAssembly and browser GPU backends. The current repo has source
contracts for Falcon-OCR browser artifacts, WebGPU/WebGL2/WASM-SIMD backend
selection, tokenizer behavior, and Cloudflare deployment boundaries.

## Evidence To Maintain

- Reproducible build command for the relevant tinygrad/Falcon-OCR program.
- Browser runtime contract tests for WASM, tokenizer, weights, and backend
  selection.
- End-to-end quality and latency results with dated hardware/browser versions.
- Prior-art review for tinygrad, ONNX Runtime Web, TensorFlow.js, WebNN, TVM,
  IREE, Pyodide, and browser WebGPU ML runtimes.

## Non-Claims

Until the evidence above exists in this repository, do not claim:

- first-ever AOT tinygrad compilation;
- first tinygrad browser runtime;
- production superiority over ONNX Runtime Web, TensorFlow.js, or Pyodide;
- production readiness of a specific Falcon-OCR browser deployment.

## Status

Treat this as a research and verification checklist. Move a claim into product
or public documentation only after it has a reproducible command, benchmark
artifact, source provenance, and dated review note.
