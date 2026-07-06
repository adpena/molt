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

- The **core tensor primitive** stays a compiler-owned stdlib package and is
  imported here exactly as a downstream user app would:
  `from tinygrad.tensor import Tensor`, `from tinygrad.dtypes import dtypes`,
  `from tinygrad.lazy import ...`, `from tinygrad.realize import ...`,
  `from tinygrad import nn`. The canonical native ML runtime primitive is
  `molt.gpu` (imported as `from molt.gpu import ...`).
- **Cross-demo** imports inside this directory are relative
  (`from .onnx_interpreter import ...`), because these modules form the
  `demos.tinygrad` package rather than submodules of the `tinygrad` core.

## Tests

`tests/` and `examples/test_falcon_ocr.py` are the demos' own tests. They run
against the moved modules via relative imports and against the compiler-owned
`tinygrad` tensor core via the external package name.
