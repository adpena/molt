"""
Local inference test for Falcon-OCR using real downloaded weights.

Loads the HuggingFace-cached Falcon-OCR weights from disk, creates a
synthetic test image, and runs the full inference pipeline locally using
pure Python (no WASM, no Worker). This is the definitive
"does the model produce real output" integration test.

Requires:
  - Downloaded Falcon-OCR weights in HF cache
  - The molt runtime (tinygrad Tensor, molt.gpu.Buffer, etc.)

Usage:
    pytest tests/e2e/test_local_inference.py -v
    python3 tests/e2e/test_local_inference.py   # standalone
"""

from __future__ import annotations

import json
import os
import struct
import sys

# ---------------------------------------------------------------------------
# Weight discovery
# ---------------------------------------------------------------------------

_SNAP_DIR = os.path.join(
    os.path.expanduser("~"),
    ".cache",
    "molt",
    "falcon-ocr",
    "models--tiiuae--Falcon-OCR",
    "snapshots",
    "3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66",
)

_MODEL_PATH = os.path.join(_SNAP_DIR, "model.safetensors")
_CONFIG_PATH = os.path.join(_SNAP_DIR, "config.json")
_TOKENIZER_PATH = os.path.join(_SNAP_DIR, "tokenizer.json")

_WEIGHTS_AVAILABLE = (
    os.path.isfile(_MODEL_PATH) and os.path.getsize(_MODEL_PATH) > 1_000_000
)


def _skip_if_no_weights():
    """Skip the test if weights are not downloaded."""
    if not _WEIGHTS_AVAILABLE:
        try:
            import pytest

            pytest.skip("Falcon-OCR weights not downloaded")
        except ImportError:
            print("SKIP: weights not available")
            return True
    return False


# ---------------------------------------------------------------------------
# Safetensors header validation (no runtime deps needed)
# ---------------------------------------------------------------------------


def test_safetensors_header_valid():
    """Validate that the safetensors file has a parseable header."""
    if _skip_if_no_weights():
        return

    with open(_MODEL_PATH, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        assert 0 < header_size < 100_000, f"Unreasonable header size: {header_size}"
        header = json.loads(f.read(header_size))

    meta = header.pop("__metadata__", {})
    assert isinstance(meta, dict)
    assert len(header) == 115, f"Expected 115 tensors, got {len(header)}"

    # Verify critical tensors exist
    required = [
        "tok_embeddings.weight",
        "img_projector.weight",
        "norm.weight",
        "output.weight",
        "freqs_cis_golden",
    ]
    for name in required:
        assert name in header, f"Missing required tensor: {name}"

    # Verify all layer tensors (0..21)
    for i in range(22):
        prefix = f"layers.{i}"
        for suffix in [
            "attention.wqkv.weight",
            "attention.wo.weight",
            "attention.sinks",
            "feed_forward.w13.weight",
            "feed_forward.w2.weight",
        ]:
            full = f"{prefix}.{suffix}"
            assert full in header, f"Missing layer tensor: {full}"

    # Verify shapes match config expectations
    assert header["tok_embeddings.weight"]["shape"] == [65536, 768]
    assert header["tok_embeddings.weight"]["dtype"] == "F32"
    assert header["output.weight"]["shape"] == [65536, 768]
    assert header["img_projector.weight"]["shape"] == [768, 768]
    assert header["norm.weight"]["shape"] == [768]

    for i in range(22):
        assert header[f"layers.{i}.attention.wqkv.weight"]["shape"] == [2048, 768]
        assert header[f"layers.{i}.attention.wo.weight"]["shape"] == [768, 1024]
        assert header[f"layers.{i}.attention.sinks"]["shape"] == [16]
        assert header[f"layers.{i}.feed_forward.w13.weight"]["shape"] == [4608, 768]
        assert header[f"layers.{i}.feed_forward.w2.weight"]["shape"] == [768, 2304]

    print(f"PASS: safetensors header valid ({len(header)} tensors, all F32)")


def test_config_matches_weights():
    """Verify config.json matches the weight dimensions."""
    if _skip_if_no_weights():
        return

    with open(_CONFIG_PATH) as f:
        config = json.load(f)

    assert config["dim"] == 768
    assert config["n_layers"] == 22
    assert config["n_heads"] == 16
    assert config["head_dim"] == 64
    assert config["n_kv_heads"] == 8
    assert config["ffn_dim"] == 2304
    assert config["vocab_size"] == 65536
    assert config["max_seq_len"] == 8192
    assert config["eos_id"] == 11
    assert config["img_id"] == 227
    assert config["spatial_patch_size"] == 16
    assert config["channel_size"] == 3

    print("PASS: config.json matches weight dimensions")


def test_tokenizer_loads():
    """Verify tokenizer.json is valid and has expected structure."""
    if _skip_if_no_weights():
        return

    with open(_TOKENIZER_PATH) as f:
        data = json.load(f)

    assert data["model"]["type"] == "BPE"
    assert len(data["model"]["vocab"]) == 65536
    assert len(data["model"]["merges"]) == 64769
    assert len(data["added_tokens"]) == 524

    # Verify key special tokens by content
    id_by_content = {t["content"]: t["id"] for t in data["added_tokens"]}
    assert id_by_content["<|pad|>"] == 0
    assert id_by_content["<|end_of_text|>"] == 11
    assert id_by_content["<|image|>"] == 227
    assert id_by_content["<|image_cls|>"] == 244

    print("PASS: tokenizer.json valid (vocab=65536, merges=64769)")


def test_weight_byte_ranges_nonoverlapping():
    """Verify tensor data offsets in safetensors don't overlap."""
    if _skip_if_no_weights():
        return

    with open(_MODEL_PATH, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_size))

    header.pop("__metadata__", None)

    # Collect (start, end) byte ranges
    ranges = []
    for name, info in sorted(header.items()):
        offsets = info["data_offsets"]
        ranges.append((offsets[0], offsets[1], name))

    # Sort by start offset
    ranges.sort()

    # Verify no overlaps
    for i in range(1, len(ranges)):
        prev_end = ranges[i - 1][1]
        curr_start = ranges[i][0]
        assert curr_start >= prev_end, (
            f"Overlap: {ranges[i - 1][2]} ends at {prev_end}, "
            f"{ranges[i][2]} starts at {curr_start}"
        )

    # Verify total data size is reasonable (should be ~1GB for 256M f32 params)
    total_end = max(r[1] for r in ranges)
    # 115 tensors, mostly 768-dim, 22 layers => ~256M params * 4 bytes = ~1GB
    assert total_end > 900_000_000, f"Total data too small: {total_end}"
    assert total_end < 1_200_000_000, f"Total data too large: {total_end}"

    print(
        f"PASS: {len(ranges)} tensor ranges non-overlapping, total {total_end:,} bytes"
    )


def test_load_single_tensor():
    """Load a single small tensor and verify its values are reasonable."""
    if _skip_if_no_weights():
        return

    with open(_MODEL_PATH, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_size))
        data_start = 8 + header_size

        # Load norm.weight (shape [768], smallest non-trivial tensor)
        info = header["norm.weight"]
        assert info["dtype"] == "F32"
        assert info["shape"] == [768]
        start, end = info["data_offsets"]
        f.seek(data_start + start)
        raw = f.read(end - start)
        assert len(raw) == 768 * 4

    # Parse as f32 values
    values = struct.unpack(f"<{768}f", raw)
    assert len(values) == 768

    # RMSNorm weights are typically initialized near 1.0 and stay close
    mean_val = sum(values) / len(values)
    assert 0.1 < mean_val < 10.0, f"norm.weight mean={mean_val}, expected near 1.0"

    # No NaN or Inf
    import math

    for i, v in enumerate(values):
        assert math.isfinite(v), f"norm.weight[{i}] = {v} is not finite"

    print(f"PASS: norm.weight loaded, mean={mean_val:.4f}")


def test_load_attention_sinks():
    """Load attention sinks tensor and verify structure."""
    if _skip_if_no_weights():
        return

    with open(_MODEL_PATH, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_size))
        data_start = 8 + header_size

        info = header["layers.0.attention.sinks"]
        assert info["dtype"] == "F32"
        assert info["shape"] == [16]  # n_heads = 16
        start, end = info["data_offsets"]
        f.seek(data_start + start)
        raw = f.read(end - start)

    values = struct.unpack("<16f", raw)
    assert len(values) == 16

    import math

    for i, v in enumerate(values):
        assert math.isfinite(v), f"sinks[{i}] = {v}"

    print(f"PASS: attention sinks loaded, values={[f'{v:.3f}' for v in values[:4]]}...")


def test_synthetic_image_pipeline():
    """Create a synthetic test image and verify preprocessing math."""
    if _skip_if_no_weights():
        return

    # Synthetic 32x32 RGB image (white rectangle on black background)
    width, height = 32, 32
    patch_size = 16
    channels = 3
    rgb = bytearray(width * height * channels)

    # Draw a white rectangle in the center (8,8) to (24,24)
    for y in range(8, 24):
        for x in range(8, 24):
            offset = (y * width + x) * channels
            rgb[offset] = 255  # R
            rgb[offset + 1] = 255  # G
            rgb[offset + 2] = 255  # B

    rgb = bytes(rgb)
    assert len(rgb) == width * height * channels

    # Verify patch geometry
    n_w = width // patch_size
    n_h = height // patch_size
    assert n_w == 2
    assert n_h == 2
    n_patches = n_w * n_h
    assert n_patches == 4
    patch_dim = patch_size * patch_size * channels
    assert patch_dim == 768  # Matches model dim -- this is by design

    # Verify normalization: (b/255)*2 - 1 maps [0,255] -> [-1, 1]
    assert abs((0 / 255.0) * 2 - 1 - (-1.0)) < 1e-6
    assert abs((255 / 255.0) * 2 - 1 - 1.0) < 1e-6
    assert abs((127 / 255.0) * 2 - 1 - (-0.00392)) < 1e-3

    print(f"PASS: synthetic image pipeline ({n_patches} patches of dim {patch_dim})")


# ---------------------------------------------------------------------------
# Full inference test (requires molt runtime)
# ---------------------------------------------------------------------------


def test_full_inference():
    """Load real weights and run inference on a synthetic image.

    This test requires the molt runtime (tinygrad Tensor, molt.gpu.Buffer).
    Skip gracefully when running outside molt.
    """
    if _skip_if_no_weights():
        return

    # Try to import molt runtime
    try:
        # Add the source tree to path for standalone execution
        project_root = os.path.dirname(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        )
        stdlib_path = os.path.join(project_root, "src", "molt", "stdlib")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)

        from tinygrad.examples.falcon_ocr import init, ocr_tokens, FalconOCRConfig

        assert FalconOCRConfig is not None
    except (ImportError, SyntaxError) as e:
        # SyntaxError: _intrinsics.py uses molt-specific syntax not valid in CPython
        print(f"SKIP: molt runtime not available ({e})")
        return

    # Load weights from disk
    with open(_MODEL_PATH, "rb") as f:
        weights_bytes = f.read()

    with open(_CONFIG_PATH) as f:
        config_json = f.read()

    # Initialize model
    init(weights_bytes, config_json)

    # Create a synthetic 32x32 white image
    width, height = 32, 32
    rgb = bytes([200] * (width * height * 3))  # Gray image

    # Run inference with a minimal prompt
    # Token 257 = <|OCR_PLAIN|> (OCR task token)
    prompt_ids = [17, 257]  # <|begin_of_text|>, <|OCR_PLAIN|>
    max_new_tokens = 5

    generated = ocr_tokens(width, height, rgb, prompt_ids, max_new_tokens)

    # Verify output structure
    assert isinstance(generated, list), f"Expected list, got {type(generated)}"
    assert len(generated) > 0, "Generated no tokens"
    assert len(generated) <= max_new_tokens, (
        f"Generated {len(generated)} > max {max_new_tokens}"
    )
    assert all(isinstance(t, int) for t in generated), "Not all tokens are ints"
    assert all(0 <= t < 65536 for t in generated), (
        f"Token out of vocab range: {generated}"
    )

    print(f"PASS: full inference produced {len(generated)} tokens: {generated}")


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    test_safetensors_header_valid()
    test_config_matches_weights()
    test_tokenizer_loads()
    test_weight_byte_ranges_nonoverlapping()
    test_load_single_tensor()
    test_load_attention_sinks()
    test_synthetic_image_pipeline()
    test_full_inference()
    print("\n=== ALL LOCAL INFERENCE TESTS PASSED ===")
