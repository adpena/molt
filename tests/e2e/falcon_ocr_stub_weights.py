"""
Deterministic stub weight generator for Falcon-OCR parity testing.

Generates small random-but-deterministic weights using the same config
structure as real Falcon-OCR but with reduced dimensions:
    - num_layers=2 instead of 22
    - dim=64 instead of 768
    - n_heads=4 instead of 16
    - head_dim=16 instead of 64
    - n_kv_heads=2 instead of 8
    - ffn_dim=128 instead of 2304
    - vocab_size=256 instead of 65536

Both the CPython+tinygrad reference path and the molt path use identical
stub weights produced by this module. The seed is fixed at 42 so every
invocation yields bit-identical weights.

Output format: SafeTensors bytes (usable with load_safetensors_bytes).
"""

from __future__ import annotations

import json
import math
import random
import struct
from typing import Dict, List, Tuple

# ---------------------------------------------------------------------------
# Stub config (small model for fast testing)
# ---------------------------------------------------------------------------

STUB_CONFIG = {
    "dim": 64,
    "n_layers": 2,
    "n_heads": 4,
    "head_dim": 16,
    "n_kv_heads": 2,
    "ffn_dim": 128,
    "vocab_size": 256,
    "max_seq_len": 128,
    "rope_theta": 10000.0,
    "norm_eps": 1e-5,
    "rms_inner_eps": 1e-6,
    "channel_size": 3,
    "spatial_patch_size": 16,
    "temporal_patch_size": 1,
    "eos_id": 11,
    "img_id": 227,
    "img_row_sep_id": 228,
    "img_start_id": 229,
    "img_end_id": 230,
    "coord_token_id": 240,
    "size_token_id": 241,
    "image_cls_token_id": 244,
    "image_reg_1_token_id": 245,
    "image_reg_2_token_id": 246,
    "image_reg_3_token_id": 247,
    "image_reg_4_token_id": 248,
    "seg_token_id": 262,
}

SEED = 42


# ---------------------------------------------------------------------------
# Deterministic random tensor generation
# ---------------------------------------------------------------------------

def _make_tensor(rng: random.Random, shape: Tuple[int, ...], scale: float = 0.02) -> List[float]:
    """Generate a flat list of floats with Xavier-like initialization."""
    n = 1
    for s in shape:
        n *= s
    # Xavier uniform: U(-a, a) where a = sqrt(6 / (fan_in + fan_out))
    # For simplicity with stub weights, use a fixed small scale.
    return [rng.gauss(0.0, scale) for _ in range(n)]


def _make_ones(shape: Tuple[int, ...]) -> List[float]:
    """Generate a flat list of 1.0 values (for norm weights)."""
    n = 1
    for s in shape:
        n *= s
    return [1.0] * n


# ---------------------------------------------------------------------------
# SafeTensors serialization
# ---------------------------------------------------------------------------

def _encode_f32(values: List[float]) -> bytes:
    """Encode floats as little-endian F32."""
    return struct.pack(f"<{len(values)}f", *values)


def _build_safetensors(tensors: Dict[str, Tuple[Tuple[int, ...], List[float]]]) -> bytes:
    """Build a SafeTensors binary blob from {name: (shape, flat_values)}.

    Each tensor is stored as F32. The format is:
        8 bytes: header length (u64 LE)
        header_len bytes: JSON header
        data bytes: concatenated tensor data
    """
    entries = {}
    data_parts = []
    offset = 0

    for name in sorted(tensors.keys()):
        shape, values = tensors[name]
        raw = _encode_f32(values)
        entries[name] = {
            "dtype": "F32",
            "shape": list(shape),
            "data_offsets": [offset, offset + len(raw)],
        }
        data_parts.append(raw)
        offset += len(raw)

    header_json = json.dumps(entries, separators=(",", ":")).encode("utf-8")
    header_len = struct.pack("<Q", len(header_json))
    return header_len + header_json + b"".join(data_parts)


# ---------------------------------------------------------------------------
# Generate stub weights
# ---------------------------------------------------------------------------

def generate_stub_weights(seed: int = SEED) -> bytes:
    """Generate deterministic stub weights as SafeTensors bytes.

    Returns bytes that can be passed directly to falcon_ocr.init().
    """
    rng = random.Random(seed)
    cfg = STUB_CONFIG
    dim = cfg["dim"]
    n_layers = cfg["n_layers"]
    n_heads = cfg["n_heads"]
    head_dim = cfg["head_dim"]
    n_kv_heads = cfg["n_kv_heads"]
    ffn_dim = cfg["ffn_dim"]
    vocab_size = cfg["vocab_size"]
    patch_size = cfg["spatial_patch_size"]
    channel_size = cfg["channel_size"]

    # Derived dimensions
    q_dim = n_heads * head_dim        # 4 * 16 = 64
    kv_dim = n_kv_heads * head_dim    # 2 * 16 = 32
    qkv_dim = q_dim + 2 * kv_dim     # 64 + 64 = 128
    patch_dim = patch_size * patch_size * channel_size  # 16 * 16 * 3 = 768

    tensors: Dict[str, Tuple[Tuple[int, ...], List[float]]] = {}

    # Token embeddings: (vocab_size, dim)
    tensors["tok_embeddings.weight"] = (
        (vocab_size, dim),
        _make_tensor(rng, (vocab_size, dim)),
    )

    # Image projector: (patch_dim, dim)
    tensors["img_projector.weight"] = (
        (patch_dim, dim),
        _make_tensor(rng, (patch_dim, dim)),
    )

    # Transformer layers
    for i in range(n_layers):
        prefix = f"layers.{i}"

        # Attention: wqkv projects dim -> qkv_dim
        tensors[f"{prefix}.attention.wqkv.weight"] = (
            (dim, qkv_dim),
            _make_tensor(rng, (dim, qkv_dim)),
        )

        # Attention: wo projects q_dim -> dim
        tensors[f"{prefix}.attention.wo.weight"] = (
            (q_dim, dim),
            _make_tensor(rng, (q_dim, dim)),
        )

        # Attention sinks: (n_heads,)
        tensors[f"{prefix}.attention.sinks"] = (
            (n_heads,),
            _make_tensor(rng, (n_heads,)),
        )

        # FFN: w13 projects dim -> 2*ffn_dim (gate + up interleaved)
        tensors[f"{prefix}.feed_forward.w13.weight"] = (
            (dim, 2 * ffn_dim),
            _make_tensor(rng, (dim, 2 * ffn_dim)),
        )

        # FFN: w2 projects ffn_dim -> dim
        tensors[f"{prefix}.feed_forward.w2.weight"] = (
            (ffn_dim, dim),
            _make_tensor(rng, (ffn_dim, dim)),
        )

    # Final norm weight: (dim,)
    tensors["norm.weight"] = (
        (dim,),
        _make_ones((dim,)),
    )

    # Output projection: (dim, vocab_size)
    tensors["output.weight"] = (
        (dim, vocab_size),
        _make_tensor(rng, (dim, vocab_size)),
    )

    return _build_safetensors(tensors)


def generate_stub_config_json() -> str:
    """Return the stub config as a JSON string."""
    return json.dumps(STUB_CONFIG)


# ---------------------------------------------------------------------------
# Synthetic test image generation
# ---------------------------------------------------------------------------

def generate_test_image(
    width: int = 32,
    height: int = 32,
    seed: int = SEED,
) -> bytes:
    """Generate a deterministic synthetic RGB image.

    The image contains a simple pattern: horizontal gradient modulated
    by vertical stripes. This is deterministic and produces known content
    for reproducible testing.

    Width and height must be multiples of the spatial patch size (16).

    Returns raw RGB bytes (height * width * 3).
    """
    if width % 16 != 0 or height % 16 != 0:
        raise ValueError(
            f"Image dimensions {width}x{height} must be multiples of 16"
        )
    rng = random.Random(seed + 1)  # Different seed from weights
    pixels = bytearray(height * width * 3)
    idx = 0
    for y in range(height):
        for x in range(width):
            # Deterministic pattern: gradient + noise
            r = int((x / max(width - 1, 1)) * 200 + rng.randint(0, 55))
            g = int((y / max(height - 1, 1)) * 200 + rng.randint(0, 55))
            b = int(((x + y) / max(width + height - 2, 1)) * 200 + rng.randint(0, 55))
            pixels[idx] = min(255, max(0, r))
            pixels[idx + 1] = min(255, max(0, g))
            pixels[idx + 2] = min(255, max(0, b))
            idx += 3
    return bytes(pixels)


# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

def verify_determinism() -> bool:
    """Verify that two calls produce identical weights."""
    w1 = generate_stub_weights()
    w2 = generate_stub_weights()
    if w1 != w2:
        return False
    i1 = generate_test_image()
    i2 = generate_test_image()
    if i1 != i2:
        return False
    return True


def describe_stub_model() -> str:
    """Return a human-readable description of the stub model."""
    cfg = STUB_CONFIG
    dim = cfg["dim"]
    n_layers = cfg["n_layers"]
    n_heads = cfg["n_heads"]
    head_dim = cfg["head_dim"]
    n_kv_heads = cfg["n_kv_heads"]
    ffn_dim = cfg["ffn_dim"]
    vocab_size = cfg["vocab_size"]
    patch_dim = cfg["spatial_patch_size"] ** 2 * cfg["channel_size"]

    # Count parameters
    tok_params = vocab_size * dim
    proj_params = patch_dim * dim
    q_dim = n_heads * head_dim
    kv_dim = n_kv_heads * head_dim
    qkv_dim = q_dim + 2 * kv_dim
    layer_params = (
        dim * qkv_dim          # wqkv
        + q_dim * dim           # wo
        + n_heads               # sinks
        + dim * 2 * ffn_dim     # w13
        + ffn_dim * dim         # w2
    )
    norm_params = dim
    output_params = dim * vocab_size
    total = tok_params + proj_params + n_layers * layer_params + norm_params + output_params

    lines = [
        "Falcon-OCR Stub Model",
        f"  dim={dim}, n_layers={n_layers}, n_heads={n_heads}, head_dim={head_dim}",
        f"  n_kv_heads={n_kv_heads}, ffn_dim={ffn_dim}, vocab_size={vocab_size}",
        f"  patch_dim={patch_dim}",
        f"  Total parameters: {total:,}",
        f"  Estimated size: {total * 4 / 1024:.1f} KB (F32)",
    ]
    return "\n".join(lines)


if __name__ == "__main__":
    print(describe_stub_model())
    print()
    assert verify_determinism(), "FAIL: stub weights are not deterministic"
    print("PASS: stub weights are deterministic")
    weights = generate_stub_weights()
    print(f"SafeTensors blob size: {len(weights):,} bytes")
    image = generate_test_image()
    print(f"Test image size: {len(image):,} bytes (32x32 RGB)")
