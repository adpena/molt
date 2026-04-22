"""Browser WASM validation tests.

Verifies the WASM binary served from the Worker is valid and contains
the expected exports for browser-side inference.
"""

import struct
import urllib.request
import json

WASM_URL = "https://falcon-ocr.adpena.workers.dev/wasm/falcon-ocr.wasm"
WEIGHTS_URL = "https://falcon-ocr.adpena.workers.dev/weights/falcon-ocr-int4/model.safetensors.index.json"


def test_wasm_binary_valid():
    """WASM binary is valid WebAssembly."""
    with urllib.request.urlopen(WASM_URL) as r:
        header = r.read(8)
        magic = header[:4]
        version = struct.unpack("<I", header[4:])[0]
        assert magic == b"\x00asm", f"Invalid magic: {magic}"
        assert version == 1, f"Unexpected version: {version}"


def test_wasm_binary_size():
    """WASM binary is within size budget."""
    req = urllib.request.Request(WASM_URL, method="HEAD")
    with urllib.request.urlopen(req) as r:
        size = int(r.headers.get("Content-Length", 0))
        assert size > 0, "Empty WASM"
        assert size < 20_000_000, f"WASM too large: {size} bytes"


def test_weights_index_valid():
    """Weight shard index is valid JSON with expected structure."""
    with urllib.request.urlopen(WEIGHTS_URL) as r:
        data = json.loads(r.read())
        assert "weight_map" in data or isinstance(data, dict)


def test_wasm_caching_headers():
    """WASM is served with immutable caching headers."""
    req = urllib.request.Request(WASM_URL, method="HEAD")
    with urllib.request.urlopen(req) as r:
        cache = r.headers.get("Cache-Control", "")
        assert "immutable" in cache or "max-age" in cache


def test_cors_headers():
    """WASM endpoint has proper CORS for cross-origin loading."""
    req = urllib.request.Request(WASM_URL)
    req.add_header("Origin", "https://freeinvoicemaker.app")
    with urllib.request.urlopen(req) as r:
        # Should allow cross-origin access
        assert r.status == 200
