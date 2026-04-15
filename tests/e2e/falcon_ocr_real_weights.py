"""
Falcon-OCR real-weight download and reference generation.

Downloads Falcon-OCR weights from HuggingFace (tiiuae/Falcon-OCR, ~300MB safetensors),
caches them locally, runs inference on a test image, and saves reference output.

Usage:
    python tests/e2e/falcon_ocr_real_weights.py --download
    python tests/e2e/falcon_ocr_real_weights.py --generate-reference
    python tests/e2e/falcon_ocr_real_weights.py --info
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import sys
import time
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

CACHE_DIR = Path.home() / ".cache" / "molt" / "falcon-ocr"
WEIGHTS_DIR = CACHE_DIR / "weights"
REFERENCE_DIR = CACHE_DIR / "reference"
MODEL_ID = "tiiuae/Falcon-OCR"
SAFETENSORS_FILENAME = "model.safetensors"

# Test image: 64x64 grayscale gradient (deterministic, no external deps)
TEST_IMAGE_WIDTH = 64
TEST_IMAGE_HEIGHT = 64

# ---------------------------------------------------------------------------
# Weight download
# ---------------------------------------------------------------------------


def _ensure_cache_dirs() -> None:
    """Create cache directories if they don't exist."""
    WEIGHTS_DIR.mkdir(parents=True, exist_ok=True)
    REFERENCE_DIR.mkdir(parents=True, exist_ok=True)


def weights_available() -> bool:
    """Check if Falcon-OCR weights are downloaded and cached."""
    safetensors_path = WEIGHTS_DIR / SAFETENSORS_FILENAME
    return safetensors_path.exists() and safetensors_path.stat().st_size > 0


def download_weights(force: bool = False) -> Path:
    """Download Falcon-OCR weights from HuggingFace Hub.

    Requires the `huggingface_hub` package. Falls back to direct HTTP
    download with urllib if huggingface_hub is not available.

    Returns the path to the downloaded safetensors file.
    """
    _ensure_cache_dirs()
    safetensors_path = WEIGHTS_DIR / SAFETENSORS_FILENAME

    if safetensors_path.exists() and not force:
        print(f"Weights already cached at {safetensors_path}")
        return safetensors_path

    try:
        from huggingface_hub import hf_hub_download

        print(f"Downloading {MODEL_ID} weights via huggingface_hub...")
        downloaded_path = hf_hub_download(
            repo_id=MODEL_ID,
            filename=SAFETENSORS_FILENAME,
            local_dir=str(WEIGHTS_DIR),
            local_dir_use_symlinks=False,
        )
        print(f"Downloaded to {downloaded_path}")
        return Path(downloaded_path)

    except ImportError:
        # Fallback: direct HTTP download via urllib
        import urllib.request

        url = f"https://huggingface.co/{MODEL_ID}/resolve/main/{SAFETENSORS_FILENAME}"
        print(f"Downloading {url} (huggingface_hub not installed, using urllib)...")

        start = time.monotonic()
        urllib.request.urlretrieve(url, str(safetensors_path))
        elapsed = time.monotonic() - start

        size_mb = safetensors_path.stat().st_size / (1024 * 1024)
        print(f"Downloaded {size_mb:.1f} MB in {elapsed:.1f}s")
        return safetensors_path


def _read_safetensors_header(path: Path) -> dict:
    """Read the metadata header from a safetensors file.

    Safetensors format:
    - 8 bytes: little-endian u64 header size
    - header_size bytes: JSON metadata
    - remainder: raw tensor data
    """
    with open(path, "rb") as f:
        header_size_bytes = f.read(8)
        if len(header_size_bytes) < 8:
            raise ValueError("Invalid safetensors file: too short for header size")
        header_size = struct.unpack("<Q", header_size_bytes)[0]
        if header_size > 100_000_000:
            raise ValueError(f"Safetensors header size suspiciously large: {header_size}")
        header_json = f.read(header_size)
        return json.loads(header_json)


def _load_tensor_from_safetensors(
    path: Path, header: dict, tensor_name: str
) -> tuple[list[float], list[int], str]:
    """Load a single tensor from a safetensors file.

    Returns (flat_data_as_floats, shape, dtype_str).
    """
    if tensor_name not in header:
        raise KeyError(f"Tensor '{tensor_name}' not found in safetensors header")

    meta = header[tensor_name]
    dtype_str = meta["dtype"]
    shape = meta["shape"]
    offsets = meta["data_offsets"]

    # Read raw bytes
    with open(path, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        data_start = 8 + header_size + offsets[0]
        data_len = offsets[1] - offsets[0]
        f.seek(data_start)
        raw = f.read(data_len)

    # Convert to float list based on dtype
    if dtype_str == "F32":
        count = len(raw) // 4
        values = list(struct.unpack(f"<{count}f", raw))
    elif dtype_str == "F16":
        import array as arr

        count = len(raw) // 2
        # Use struct to unpack f16 as u16, then convert
        u16_values = struct.unpack(f"<{count}H", raw)
        values = [_f16_to_f32(v) for v in u16_values]
    elif dtype_str == "BF16":
        count = len(raw) // 2
        u16_values = struct.unpack(f"<{count}H", raw)
        values = [_bf16_to_f32(v) for v in u16_values]
    else:
        raise ValueError(f"Unsupported dtype: {dtype_str}")

    return values, shape, dtype_str


def _f16_to_f32(bits: int) -> float:
    """Convert IEEE 754 half-precision bits to Python float."""
    sign = (bits >> 15) & 1
    exponent = (bits >> 10) & 0x1F
    mantissa = bits & 0x3FF

    if exponent == 0:
        if mantissa == 0:
            return (-1.0) ** sign * 0.0
        # Subnormal
        return (-1.0) ** sign * (mantissa / 1024.0) * 2.0 ** (-14)
    elif exponent == 31:
        if mantissa == 0:
            return float("-inf") if sign else float("inf")
        return float("nan")
    else:
        return (-1.0) ** sign * (1.0 + mantissa / 1024.0) * 2.0 ** (exponent - 15)


def _bf16_to_f32(bits: int) -> float:
    """Convert bfloat16 bits to Python float."""
    # bfloat16 is the upper 16 bits of float32
    f32_bits = bits << 16
    return struct.unpack("f", struct.pack("I", f32_bits))[0]


# ---------------------------------------------------------------------------
# Test image generation
# ---------------------------------------------------------------------------


def generate_test_image_bytes() -> bytes:
    """Generate a deterministic 64x64 grayscale test image as raw bytes.

    The image is a diagonal gradient pattern that exercises edge detection
    and spatial feature extraction pathways in OCR models.
    """
    pixels = bytearray(TEST_IMAGE_WIDTH * TEST_IMAGE_HEIGHT)
    for y in range(TEST_IMAGE_HEIGHT):
        for x in range(TEST_IMAGE_WIDTH):
            # Diagonal gradient with high-frequency detail
            base = ((x + y) * 255) // (TEST_IMAGE_WIDTH + TEST_IMAGE_HEIGHT - 2)
            # Add checkerboard detail for spatial frequency testing
            detail = 20 if (x // 4 + y // 4) % 2 == 0 else 0
            pixels[y * TEST_IMAGE_WIDTH + x] = min(255, base + detail)
    return bytes(pixels)


# ---------------------------------------------------------------------------
# Reference generation
# ---------------------------------------------------------------------------


def generate_reference(max_tokens: int = 16) -> Optional[Path]:
    """Run inference with real weights and save reference output.

    Returns path to the reference JSON file, or None if weights unavailable.
    """
    if not weights_available():
        print("Weights not downloaded. Run with --download first.")
        return None

    _ensure_cache_dirs()

    safetensors_path = WEIGHTS_DIR / SAFETENSORS_FILENAME
    header = _read_safetensors_header(safetensors_path)

    # Filter out __metadata__ key
    tensor_names = [k for k in header.keys() if k != "__metadata__"]
    total_params = 0
    for name in tensor_names:
        meta = header[name]
        shape = meta["shape"]
        count = 1
        for s in shape:
            count *= s
        total_params += count

    reference = {
        "model_id": MODEL_ID,
        "safetensors_file": str(safetensors_path),
        "file_size_bytes": safetensors_path.stat().st_size,
        "file_sha256": hashlib.sha256(safetensors_path.read_bytes()).hexdigest(),
        "num_tensors": len(tensor_names),
        "total_parameters": total_params,
        "tensor_names": sorted(tensor_names),
        "test_image": {
            "width": TEST_IMAGE_WIDTH,
            "height": TEST_IMAGE_HEIGHT,
            "format": "grayscale_raw",
            "sha256": hashlib.sha256(generate_test_image_bytes()).hexdigest(),
        },
        "inference": {
            "status": "pending",
            "note": (
                "Full inference requires molt runtime with tinygrad Tensor API. "
                "This reference captures the weight metadata for parity validation. "
                "Inference output will be populated when the runtime supports "
                "Falcon-OCR end-to-end."
            ),
            "max_tokens": max_tokens,
            "logits": None,
            "tokens": None,
            "time_to_first_token_ms": None,
            "tokens_per_second": None,
        },
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }

    ref_path = REFERENCE_DIR / "falcon_ocr_reference.json"
    with open(ref_path, "w") as f:
        json.dump(reference, f, indent=2)
    print(f"Reference saved to {ref_path}")
    return ref_path


def print_info() -> None:
    """Print information about cached weights and reference data."""
    print(f"Cache directory: {CACHE_DIR}")
    print(f"Weights directory: {WEIGHTS_DIR}")
    print(f"Reference directory: {REFERENCE_DIR}")
    print()

    if weights_available():
        safetensors_path = WEIGHTS_DIR / SAFETENSORS_FILENAME
        size_mb = safetensors_path.stat().st_size / (1024 * 1024)
        print(f"Weights: AVAILABLE ({size_mb:.1f} MB)")

        header = _read_safetensors_header(safetensors_path)
        tensor_names = [k for k in header.keys() if k != "__metadata__"]
        print(f"  Tensors: {len(tensor_names)}")

        # Show first few tensor names and shapes
        for name in sorted(tensor_names)[:5]:
            meta = header[name]
            print(f"  {name}: shape={meta['shape']} dtype={meta['dtype']}")
        if len(tensor_names) > 5:
            print(f"  ... and {len(tensor_names) - 5} more")
    else:
        print("Weights: NOT DOWNLOADED")
        print(f"  Run: python {__file__} --download")

    print()
    ref_path = REFERENCE_DIR / "falcon_ocr_reference.json"
    if ref_path.exists():
        with open(ref_path) as f:
            ref = json.load(f)
        print(f"Reference: AVAILABLE (generated {ref.get('generated_at', 'unknown')})")
        print(f"  Tensors: {ref.get('num_tensors', '?')}")
        print(f"  Parameters: {ref.get('total_parameters', '?'):,}")
        print(f"  Inference: {ref['inference']['status']}")
    else:
        print("Reference: NOT GENERATED")
        print(f"  Run: python {__file__} --generate-reference")


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Falcon-OCR real-weight test infrastructure"
    )
    parser.add_argument(
        "--download",
        action="store_true",
        help="Download Falcon-OCR weights from HuggingFace",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Force re-download even if weights exist",
    )
    parser.add_argument(
        "--generate-reference",
        action="store_true",
        help="Generate reference output from real weights",
    )
    parser.add_argument(
        "--info",
        action="store_true",
        help="Show info about cached weights and reference",
    )

    args = parser.parse_args()

    if args.download:
        download_weights(force=args.force)
    elif args.generate_reference:
        generate_reference()
    elif args.info:
        print_info()
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
