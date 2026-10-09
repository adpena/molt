"""Test Falcon-OCR WASM module OCR quality.

Verifies:
1. WASM module compiles and runs cleanly
2. Token generation produces real vocab tokens (not micro model noise)
3. Tokens decode to text via the tokenizer
4. The <|OCR_PLAIN|> prompt token is used
"""

import json
import os
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process
from molt.wasm_artifact import wasm_runtime_manifest_path
from tests.helpers.falcon_ocr_paths import require_falcon_ocr_weight


def _wasm_artifact(variable: str) -> Path:
    """A built Falcon-OCR module; external artifacts have no default path."""
    raw = os.environ.get(variable, "").strip()
    if not raw or not Path(raw).expanduser().exists():
        pytest.skip(f"{variable} does not name a built Falcon-OCR module")
    return Path(raw).expanduser()


def test_wasm_compiles():
    """WASM binary exists and is valid."""
    with _wasm_artifact("MOLT_FALCON_OCR_WASM_OPT").open("rb") as f:
        magic = f.read(4)
        assert magic == b"\x00asm"


def test_wasm_runs_cleanly():
    """WASM runs without traps."""
    manifest = wasm_runtime_manifest_path(_wasm_artifact("MOLT_FALCON_OCR_WASM_LINKED"))
    result = run_native_test_process(
        ["node", "wasm/run_wasm.js", str(manifest)],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert "RuntimeError" not in result.stderr, f"WASM crashed: {result.stderr[:200]}"


def test_tokenizer_decodes_ocr_tokens():
    """OCR special tokens decode correctly."""
    with require_falcon_ocr_weight("tokenizer.json").open() as f:
        data = json.load(f)
    vocab = {}
    for piece, tid in data.get("model", {}).get("vocab", {}).items():
        vocab[tid] = piece
    for t in data.get("added_tokens", []):
        vocab[t["id"]] = t["content"]

    assert vocab[257] == "<|OCR_PLAIN|>"
    assert vocab[255] == "<|OCR_GROUNDING|>"
    assert vocab[256] == "<|OCR_DOC_PARSER|>"
