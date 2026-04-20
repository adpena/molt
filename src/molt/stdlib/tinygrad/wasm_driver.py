"""
WASM entry point for Falcon-OCR inference.

Compiled with: molt build wasm_driver.py --target wasm

Exports two functions consumed by the browser-side loader and the
Cloudflare Worker:

    init(weights_bytes, config_json)
        Load SafeTensors weights and model configuration.

    ocr_tokens(width, height, rgb, prompt_ids, max_new_tokens)
        Run OCR on a single image. Returns generated token IDs.

This module is intentionally minimal — it delegates all inference logic
to falcon_ocr.py and exists solely as the WASM compilation target.
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic
_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from molt.stdlib.tinygrad.examples.falcon_ocr import (
    init as _falcon_init,
    ocr_tokens as _falcon_ocr_tokens,
)


def init(weights_bytes: bytes, config_json: str) -> None:
    """Initialize the Falcon-OCR model from SafeTensors weights and JSON config.

    Must be called exactly once before any ocr_tokens() call.  Calling
    init() a second time re-initializes the model (useful for hot-reload
    in the Cloudflare Worker).

    Args:
        weights_bytes: Raw SafeTensors file content.
        config_json:   JSON string matching the FalconOCRConfig schema.
    """
    _falcon_init(weights_bytes, config_json)


def ocr_tokens(
    width: int,
    height: int,
    rgb: bytes,
    prompt_ids: list[int],
    max_new_tokens: int = 512,
) -> list[int]:
    """Run OCR inference on a single image.

    Args:
        width:          Image width in pixels (must be a multiple of patch_size).
        height:         Image height in pixels (must be a multiple of patch_size).
        rgb:            Raw RGB pixel data, row-major, 3 bytes per pixel.
        prompt_ids:     Tokenized prompt prefix (e.g. the OCR task instruction).
        max_new_tokens: Maximum number of tokens to generate.

    Returns:
        List of generated token IDs (excluding the prompt prefix).

    Raises:
        RuntimeError: If init() has not been called.
        ValueError:   If rgb length does not match width * height * 3.
    """
    return _falcon_ocr_tokens(width, height, rgb, prompt_ids, max_new_tokens)
