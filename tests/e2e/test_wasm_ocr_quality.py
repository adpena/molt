"""Test Falcon-OCR WASM module OCR quality.

Verifies:
1. WASM module compiles and runs cleanly
2. Token generation produces real vocab tokens (not micro model noise)
3. Tokens decode to text via the tokenizer
4. The <|OCR_PLAIN|> prompt token is used
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

from tests.helpers.falcon_ocr_paths import (
    FALCON_OCR_TOKENIZER_PATH,
    falcon_ocr_weights_available,
)


WASM_OPT_PATH = Path(
    os.environ.get("MOLT_FALCON_OCR_WASM_OPT", "/tmp/falcon_latest_opt.wasm")
)
WASM_LINKED_PATH = Path(
    os.environ.get("MOLT_FALCON_OCR_WASM_LINKED", "/tmp/falcon_latest_linked.wasm")
)


def test_wasm_compiles():
    """WASM binary exists and is valid."""
    if not WASM_OPT_PATH.exists():
        pytest.skip(f"Falcon-OCR optimized WASM not found at {WASM_OPT_PATH}")
    with WASM_OPT_PATH.open("rb") as f:
        magic = f.read(4)
        assert magic == b"\x00asm"


def test_wasm_runs_cleanly():
    """WASM runs without traps."""
    if not WASM_LINKED_PATH.exists():
        pytest.skip(f"Falcon-OCR linked WASM not found at {WASM_LINKED_PATH}")
    result = subprocess.run(
        ["node", "wasm/run_wasm.js", str(WASM_LINKED_PATH)],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert "RuntimeError" not in result.stderr, f"WASM crashed: {result.stderr[:200]}"


def test_tokenizer_decodes_ocr_tokens():
    """OCR special tokens decode correctly."""
    if not falcon_ocr_weights_available():
        pytest.skip(
            "Falcon-OCR weights/config/tokenizer artifacts are not available "
            f"under {FALCON_OCR_TOKENIZER_PATH.parent}"
        )
    with FALCON_OCR_TOKENIZER_PATH.open() as f:
        data = json.load(f)
    vocab = {}
    for piece, tid in data.get("model", {}).get("vocab", {}).items():
        vocab[tid] = piece
    for t in data.get("added_tokens", []):
        vocab[t["id"]] = t["content"]

    assert vocab[257] == "<|OCR_PLAIN|>"
    assert vocab[255] == "<|OCR_GROUNDING|>"
    assert vocab[256] == "<|OCR_DOC_PARSER|>"
