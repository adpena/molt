# molt tinygrad demos

Application, model-serving, and driver code that **consumes** the molt tensor
primitive. This directory is **not** part of the compiler substrate: nothing
under `src/molt/` imports it, and it is not shipped in the compiler wheel.

It is kept self-contained so it can be extracted wholesale into a future
standalone `molt-demos` repository without touching the compiler.

## What lives here

Falcon-OCR (`examples/falcon_ocr.py`, `wasm_driver.py`, `wasm_manifest.json`,
`tokenizer.py`, `model_config.py`), PaddleOCR (`paddleocr.py`,
`paddleocr_driver.py`, `paddleocr_bench.py`, `onnx_interpreter.py`), Whisper
(`whisper_demo.py`), openpilot (`openpilot_demo.py`), and the speculative /
attention / quantization / KV-cache algorithm demos (`mirror_sd.py`,
`eagle.py`, `ddtree.py`, `flash_attention.py`, `speculative.py`,
`tree_attention.py`, `kv_cache.py`, `turbo_quant.py`, `dflash.py`), plus the
invoice template apps (`template_extractor.py`, `nl_template_filler.py`).

## Import model

- The canonical Molt-owned GPU/tensor primitive is `molt.gpu` (imported as
  `from molt.gpu import ...`). Production `tinygrad` support must compile
  upstream tinygrad Python and extensions through package/import custody, with
  Molt GPU using tinygrad's primitive model for Molt-owned compiler/runtime
  primitives.
- `reference_stdlib/tinygrad/` is a quarantined copy of the former Molt-owned
  tinygrad compatibility package. It is retained for research, science,
  regression archaeology, and reference comparison only; it is not shipped as a
  compiler stdlib package. Tests that need it load it explicitly through
  `tests/helpers/tinygrad_stdlib_loader.py`.
- Demos that import `tinygrad` are application-level probes. They must not add
  package semantics under `src/molt/stdlib`; missing package/toolchain behavior
  belongs in upstream package custody or a fail-closed diagnostic.
- **Cross-demo** imports inside this directory are relative
  (`from .onnx_interpreter import ...`), because these modules form the
  `demos.tinygrad` package rather than submodules of the `tinygrad` core.

## Tests

`tests/` and `examples/test_falcon_ocr.py` are the demos' own tests. They run
against the moved modules via relative imports and against the compiler-owned
`tinygrad` tensor core via the external package name.
