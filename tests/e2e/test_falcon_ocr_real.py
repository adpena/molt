"""
Falcon-OCR real-weight inference tests.

These tests exercise real Falcon-OCR weights (tiiuae/Falcon-OCR, ~300MB).
They are skipped if weights are not downloaded.

To download weights:
    python tests/e2e/falcon_ocr_real_weights.py --download

Run:
    python -m pytest tests/e2e/test_falcon_ocr_real.py -v
"""

from __future__ import annotations

import json
import os
import struct
import sys
import time

import pytest

# Ensure project root is importable
_project_root = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)

from tests.e2e.falcon_ocr_real_weights import (
    CACHE_DIR,
    REFERENCE_DIR,
    WEIGHTS_DIR,
    SAFETENSORS_FILENAME,
    generate_test_image_bytes,
    weights_available,
    _read_safetensors_header,
    _load_tensor_from_safetensors,
)

# ---------------------------------------------------------------------------
# Skip condition
# ---------------------------------------------------------------------------

_SKIP_REASON = (
    "Falcon-OCR weights not downloaded. "
    "Run: python tests/e2e/falcon_ocr_real_weights.py --download"
)

pytestmark = pytest.mark.skipif(not weights_available(), reason=_SKIP_REASON)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def safetensors_path():
    """Path to the cached safetensors file."""
    return WEIGHTS_DIR / SAFETENSORS_FILENAME


@pytest.fixture(scope="module")
def safetensors_header(safetensors_path):
    """Parsed safetensors header metadata."""
    return _read_safetensors_header(safetensors_path)


@pytest.fixture(scope="module")
def test_image():
    """Deterministic test image bytes."""
    return generate_test_image_bytes()


# ---------------------------------------------------------------------------
# Test: weight loading
# ---------------------------------------------------------------------------


class TestWeightLoading:
    """Tests that Falcon-OCR weights load correctly."""

    def test_safetensors_file_exists(self, safetensors_path):
        """The safetensors file exists and is non-empty."""
        assert safetensors_path.exists()
        assert safetensors_path.stat().st_size > 0

    def test_header_parses(self, safetensors_header):
        """The safetensors header is valid JSON with tensor metadata."""
        assert isinstance(safetensors_header, dict)
        # Should have at least some tensors
        tensor_names = [k for k in safetensors_header if k != "__metadata__"]
        assert len(tensor_names) > 0

    def test_all_tensors_have_required_fields(self, safetensors_header):
        """Every tensor entry has dtype, shape, and data_offsets."""
        for name, meta in safetensors_header.items():
            if name == "__metadata__":
                continue
            assert "dtype" in meta, f"Tensor '{name}' missing dtype"
            assert "shape" in meta, f"Tensor '{name}' missing shape"
            assert "data_offsets" in meta, f"Tensor '{name}' missing data_offsets"

    def test_tensor_dtypes_are_supported(self, safetensors_header):
        """All tensor dtypes are types we support (F32, F16, BF16)."""
        supported_dtypes = {"F32", "F16", "BF16", "I32", "I64", "U8"}
        for name, meta in safetensors_header.items():
            if name == "__metadata__":
                continue
            assert meta["dtype"] in supported_dtypes, (
                f"Tensor '{name}' has unsupported dtype: {meta['dtype']}"
            )

    def test_first_tensor_loads(self, safetensors_path, safetensors_header):
        """Can load at least one tensor from the file."""
        tensor_names = [k for k in safetensors_header if k != "__metadata__"]
        # Pick the smallest tensor for speed
        smallest_name = min(
            tensor_names,
            key=lambda n: (
                safetensors_header[n]["data_offsets"][1]
                - safetensors_header[n]["data_offsets"][0]
            ),
        )
        values, shape, dtype_str = _load_tensor_from_safetensors(
            safetensors_path, safetensors_header, smallest_name
        )
        expected_count = 1
        for s in shape:
            expected_count *= s
        assert len(values) == expected_count
        assert all(isinstance(v, float) for v in values)


# ---------------------------------------------------------------------------
# Test: forward pass (stub — requires full molt runtime)
# ---------------------------------------------------------------------------


class TestForwardPass:
    """Tests that a forward pass produces valid output.

    These tests require the full molt tinygrad runtime to be functional.
    They are structured to work once the runtime supports Falcon-OCR end-to-end.
    """

    def _molt_tinygrad_available(self) -> bool:
        """Check if molt tinygrad Tensor API is importable."""
        try:
            stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
            if stdlib_path not in sys.path:
                sys.path.insert(0, stdlib_path)
            import tinygrad.tensor  # noqa: F401
            return True
        except (ImportError, ModuleNotFoundError):
            return False

    @pytest.mark.skipif(
        True,  # Will be updated when molt runtime supports Falcon-OCR
        reason="Full molt tinygrad runtime not yet available for Falcon-OCR inference",
    )
    def test_forward_produces_valid_logits(self, safetensors_path, test_image):
        """Forward pass produces logits with correct shape and finite values."""
        # This test will be activated when the full inference pipeline is ready.
        # Expected structure:
        #   model = FalconOCR.from_safetensors(safetensors_path)
        #   logits = model.forward(test_image)
        #   assert logits.shape[0] == 1  # batch
        #   assert logits.shape[-1] == model.config.vocab_size
        #   assert all(math.isfinite(v) for v in logits.flatten())
        pass

    @pytest.mark.skipif(
        True,
        reason="Full molt tinygrad runtime not yet available for Falcon-OCR inference",
    )
    def test_greedy_decode_produces_text(self, safetensors_path, test_image):
        """Greedy decoding produces non-empty text output."""
        # Expected structure:
        #   model = FalconOCR.from_safetensors(safetensors_path)
        #   tokens = model.greedy_decode(test_image, max_tokens=16)
        #   text = model.tokenizer.decode(tokens)
        #   assert len(text) > 0
        #   assert isinstance(text, str)
        pass


# ---------------------------------------------------------------------------
# Test: performance measurements
# ---------------------------------------------------------------------------


class TestPerformance:
    """Performance measurement tests.

    These record performance characteristics for regression tracking.
    They do not enforce specific thresholds (hardware-dependent) but
    ensure measurements are captured.
    """

    def test_weight_load_time(self, safetensors_path, safetensors_header):
        """Measure time to load the safetensors header."""
        start = time.monotonic()
        _read_safetensors_header(safetensors_path)
        elapsed_ms = (time.monotonic() - start) * 1000
        print(f"\n  Header parse time: {elapsed_ms:.1f}ms")
        # Header parse should be fast (< 100ms for any reasonable model)
        assert elapsed_ms < 5000, f"Header parse took {elapsed_ms:.1f}ms (>5s)"

    def test_single_tensor_load_time(self, safetensors_path, safetensors_header):
        """Measure time to load one tensor from disk."""
        tensor_names = [k for k in safetensors_header if k != "__metadata__"]
        # Find a medium-sized tensor (not too small, not too large)
        sorted_by_size = sorted(
            tensor_names,
            key=lambda n: (
                safetensors_header[n]["data_offsets"][1]
                - safetensors_header[n]["data_offsets"][0]
            ),
        )
        target = sorted_by_size[len(sorted_by_size) // 2]

        start = time.monotonic()
        values, shape, dtype_str = _load_tensor_from_safetensors(
            safetensors_path, safetensors_header, target
        )
        elapsed_ms = (time.monotonic() - start) * 1000

        count = 1
        for s in shape:
            count *= s
        size_mb = count * 4 / (1024 * 1024)  # approximate as f32
        print(f"\n  Tensor '{target}': {count:,} elements, ~{size_mb:.1f}MB, {elapsed_ms:.1f}ms")
        assert len(values) == count

    def test_test_image_generation_time(self):
        """Measure test image generation time (should be negligible)."""
        start = time.monotonic()
        img = generate_test_image_bytes()
        elapsed_us = (time.monotonic() - start) * 1_000_000
        print(f"\n  Test image generation: {elapsed_us:.0f}us ({len(img)} bytes)")
        assert len(img) == 64 * 64
        # Should be < 10ms
        assert elapsed_us < 10_000


# ---------------------------------------------------------------------------
# Test: reference data
# ---------------------------------------------------------------------------


class TestReferenceData:
    """Tests for reference data generation and consistency."""

    def test_reference_can_be_generated(self, safetensors_path, safetensors_header):
        """Reference JSON can be generated from weights."""
        from tests.e2e.falcon_ocr_real_weights import generate_reference

        ref_path = generate_reference()
        assert ref_path is not None
        assert ref_path.exists()

        with open(ref_path) as f:
            ref = json.load(f)
        assert ref["model_id"] == "tiiuae/Falcon-OCR"
        assert ref["num_tensors"] > 0
        assert ref["total_parameters"] > 0
        assert ref["test_image"]["width"] == 64
        assert ref["test_image"]["height"] == 64
