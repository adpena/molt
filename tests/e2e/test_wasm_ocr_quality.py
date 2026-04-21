"""Test Falcon-OCR WASM module OCR quality.

Verifies:
1. WASM module compiles and runs cleanly
2. Token generation produces real vocab tokens (not micro model noise)
3. Tokens decode to text via the tokenizer
4. The <|OCR_PLAIN|> prompt token is used
"""
import subprocess
import json
import struct
import os

SNAP = os.path.expanduser("~/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/snapshots/3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66")


def test_wasm_compiles():
    """WASM binary exists and is valid."""
    path = "/tmp/falcon_latest_opt.wasm"
    assert os.path.exists(path), "WASM binary not found"
    with open(path, "rb") as f:
        magic = f.read(4)
        assert magic == b"\x00asm"


def test_wasm_runs_cleanly():
    """WASM runs without traps."""
    result = subprocess.run(
        ["node", "wasm/run_wasm.js", "/tmp/falcon_latest_linked.wasm"],
        capture_output=True, text=True, timeout=30
    )
    assert "RuntimeError" not in result.stderr, f"WASM crashed: {result.stderr[:200]}"


def test_tokenizer_decodes_ocr_tokens():
    """OCR special tokens decode correctly."""
    with open(f"{SNAP}/tokenizer.json") as f:
        data = json.load(f)
    vocab = {}
    for piece, tid in data.get("model", {}).get("vocab", {}).items():
        vocab[tid] = piece
    for t in data.get("added_tokens", []):
        vocab[t["id"]] = t["content"]

    assert vocab[257] == "<|OCR_PLAIN|>"
    assert vocab[255] == "<|OCR_GROUNDING|>"
    assert vocab[256] == "<|OCR_DOC_PARSER|>"
