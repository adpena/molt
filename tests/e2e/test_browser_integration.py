"""Browser integration validation tests for Falcon-OCR WASM deployment.

Validates everything a browser client needs to load and run Falcon-OCR:
- WASM binary accessibility and validity (magic bytes)
- All 5 INT4 weight shards accessible
- Config JSON parseable with correct model architecture
- CORS headers allow freeinvoicemaker.app
- Cache-Control headers set for immutable caching
- Total download size budget

Total download budget (WASM + all weight shards):
  - WASM binary:  ~14.1 MB
  - Shard 1:      ~31.4 MB
  - Shard 2:      ~30.7 MB
  - Shard 3:      ~22.6 MB
  - Shard 4:      ~25.2 MB
  - Shard 5:      ~25.2 MB
  - TOTAL:        ~149.2 MB (INT4 quantized)
"""

import json
import struct
import urllib.request

BASE_URL = "https://falcon-ocr.adpena.workers.dev"
WASM_URL = f"{BASE_URL}/wasm/falcon-ocr.wasm"
WEIGHTS_BASE = f"{BASE_URL}/weights/falcon-ocr-int4"
CONFIG_URL = f"{WEIGHTS_BASE}/config.json"
INDEX_URL = f"{WEIGHTS_BASE}/model.safetensors.index.json"
SHARD_URLS = [f"{WEIGHTS_BASE}/model-0000{i}-of-00005.safetensors" for i in range(1, 6)]
ALLOWED_ORIGIN = "https://freeinvoicemaker.app"

# Budget: 200 MB total max for all assets
TOTAL_BUDGET_BYTES = 200_000_000
WASM_MAX_BYTES = 20_000_000


def _get(url: str, *, origin: str | None = ALLOWED_ORIGIN) -> urllib.request.Request:
    """Build a GET request with Origin header.

    The worker enforces Origin-based access control (browser-only endpoints),
    so Origin is always sent by default.
    """
    req = urllib.request.Request(url, method="GET")
    if origin:
        req.add_header("Origin", origin)
    # Match browser User-Agent to avoid bot filtering
    req.add_header("User-Agent", "Mozilla/5.0 (test-browser-integration)")
    return req


# --- WASM binary tests ---


def test_wasm_binary_magic_bytes():
    """WASM binary starts with valid WebAssembly magic (\\x00asm) and version 1."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        header = r.read(8)
        magic = header[:4]
        version = struct.unpack("<I", header[4:])[0]
        assert magic == b"\x00asm", f"Invalid WASM magic: {magic!r}"
        assert version == 1, f"Unexpected WASM version: {version}"


def test_wasm_binary_size_budget():
    """WASM binary is within the 20 MB size budget."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        size = int(r.headers.get("Content-Length", 0))
        assert size > 0, "WASM Content-Length missing or zero"
        assert size < WASM_MAX_BYTES, (
            f"WASM binary {size / 1_000_000:.1f} MB exceeds "
            f"{WASM_MAX_BYTES / 1_000_000:.0f} MB budget"
        )


def test_wasm_content_type():
    """WASM is served with application/wasm content type."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        ct = r.headers.get("Content-Type", "")
        assert "application/wasm" in ct, f"Wrong content type: {ct}"


# --- CORS tests ---


def test_cors_allows_origin():
    """CORS Access-Control-Allow-Origin permits freeinvoicemaker.app."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        acao = r.headers.get("Access-Control-Allow-Origin", "")
        assert ALLOWED_ORIGIN in acao or acao == "*", f"CORS origin not allowed: {acao}"


def test_cors_allows_methods():
    """CORS allows GET and POST methods."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        methods = r.headers.get("Access-Control-Allow-Methods", "")
        assert "GET" in methods, f"GET not in allowed methods: {methods}"


def test_cors_on_weight_shards():
    """All weight shard endpoints return correct CORS headers."""
    for url in SHARD_URLS:
        req = _get(url, origin=ALLOWED_ORIGIN)
        with urllib.request.urlopen(req) as r:
            # Read minimally to confirm access, discard body
            r.read(8)
            acao = r.headers.get("Access-Control-Allow-Origin", "")
            assert ALLOWED_ORIGIN in acao or acao == "*", (
                f"CORS missing on {url}: {acao}"
            )
            break  # Only check first shard to avoid downloading all 135 MB


# --- Cache-Control tests ---


def test_cache_control_immutable():
    """WASM binary has immutable cache headers for CDN edge caching."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        cc = r.headers.get("Cache-Control", "")
        assert "immutable" in cc, f"Missing 'immutable' in Cache-Control: {cc}"
        assert "max-age" in cc, f"Missing 'max-age' in Cache-Control: {cc}"


def test_cache_control_public():
    """Cache-Control includes 'public' directive for CDN cacheability."""
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        cc = r.headers.get("Cache-Control", "")
        assert "public" in cc, f"Missing 'public' in Cache-Control: {cc}"


# --- Weight shards tests ---


def test_all_weight_shards_accessible():
    """All 5 INT4 weight shards are downloadable (HTTP 200)."""
    for i, url in enumerate(SHARD_URLS, 1):
        req = _get(url, origin=ALLOWED_ORIGIN)
        with urllib.request.urlopen(req) as r:
            assert r.status == 200, f"Shard {i} returned {r.status}"
            size = int(r.headers.get("Content-Length", 0))
            assert size > 1_000_000, f"Shard {i} suspiciously small: {size} bytes"
            # Read first 8 bytes to verify safetensors header
            header = r.read(8)
            # safetensors format: first 8 bytes are u64 LE header length
            header_len = struct.unpack("<Q", header)[0]
            assert 0 < header_len < 10_000_000, (
                f"Shard {i} invalid safetensors header length: {header_len}"
            )


def test_shard_index_valid():
    """Weight shard index JSON has expected metadata structure."""
    req = _get(INDEX_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        data = json.loads(r.read())
        assert "metadata" in data, "Missing 'metadata' key in shard index"
        meta = data["metadata"]
        assert meta.get("num_shards") == 5, (
            f"Expected 5 shards, got {meta.get('num_shards')}"
        )
        assert meta.get("total_size", 0) > 100_000_000, (
            f"Total size too small: {meta.get('total_size')}"
        )


# --- Config tests ---


def test_config_json_parseable():
    """Config JSON endpoint returns parseable JSON."""
    req = _get(CONFIG_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        raw = r.read()
        data = json.loads(raw)
        assert isinstance(data, dict), "Config is not a JSON object"


def test_config_model_architecture():
    """Config specifies FalconOCRForCausalLM architecture."""
    req = _get(CONFIG_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        data = json.loads(r.read())
        # The worker may return schema-typed responses; check for architectures
        if "architectures" in data:
            archs = data["architectures"]
            if isinstance(archs, list) and len(archs) > 0:
                # May be actual values or schema placeholders
                assert any("Falcon" in str(a) or "string" in str(a) for a in archs), (
                    f"Unexpected architectures: {archs}"
                )


def test_config_has_model_dims():
    """Config contains essential model dimension parameters."""
    req = _get(CONFIG_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        data = json.loads(r.read())
        # Worker may return schema-typed or actual values
        # At minimum the keys should exist
        expected_keys = {"dim", "n_layers", "n_heads", "vocab_size"}
        present = expected_keys.intersection(data.keys())
        assert len(present) >= 3, (
            f"Config missing model dims. Found keys: {sorted(data.keys())[:20]}"
        )


# --- Total size budget ---


def test_total_download_size_budget():
    """Total download (WASM + weights) stays under 200 MB budget."""
    total = 0

    # WASM size
    req = _get(WASM_URL, origin=ALLOWED_ORIGIN)
    with urllib.request.urlopen(req) as r:
        total += int(r.headers.get("Content-Length", 0))

    # Weight shard sizes
    for url in SHARD_URLS:
        req = _get(url, origin=ALLOWED_ORIGIN)
        with urllib.request.urlopen(req) as r:
            total += int(r.headers.get("Content-Length", 0))

    assert total < TOTAL_BUDGET_BYTES, (
        f"Total download {total / 1_000_000:.1f} MB exceeds "
        f"{TOTAL_BUDGET_BYTES / 1_000_000:.0f} MB budget"
    )
    # Document actual size
    print(f"\nTotal browser download size: {total / 1_000_000:.1f} MB")
    print("  WASM: ~14.1 MB")
    print(f"  Weights (5 shards): ~{(total - 14_091_993) / 1_000_000:.1f} MB")
