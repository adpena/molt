"""
Model configuration parser for Falcon-OCR.

Parses config.json and model_args.json from the downloaded HuggingFace
model checkpoint and validates them against the FalconOCRConfig class.

Public API:
    load_config(path: str) -> dict
    load_model_args(path: str) -> dict
    validate_config(config: dict, model_args: dict | None) -> list[str]
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

import json
import os


# Canonical config fields and their expected types/ranges.
# Each entry: (field_name, expected_type, optional_expected_value)
_CANONICAL_FIELDS: list[tuple[str, type, object]] = [
    ("dim", int, 768),
    ("n_layers", int, 22),
    ("n_heads", int, 16),
    ("head_dim", int, 64),
    ("n_kv_heads", int, 8),
    ("ffn_dim", int, 2304),
    ("vocab_size", int, 65536),
    ("max_seq_len", int, 8192),
    ("rope_theta", (int, float), 10000),
    ("norm_eps", float, 1e-5),
    ("channel_size", int, 3),
    ("spatial_patch_size", int, 16),
    ("temporal_patch_size", int, 1),
    ("eos_id", int, 11),
    ("img_id", int, 227),
    ("img_end_id", int, 230),
    ("image_cls_token_id", int, 244),
    ("image_reg_1_token_id", int, 245),
    ("image_reg_2_token_id", int, 246),
    ("image_reg_3_token_id", int, 247),
    ("image_reg_4_token_id", int, 248),
]


def load_config(path: str) -> dict:
    """Load config.json from a model directory or file path.

    Args:
        path: Path to config.json or the directory containing it.

    Returns:
        Parsed configuration dictionary.
    """
    if os.path.isdir(path):
        path = os.path.join(path, "config.json")
    with open(path) as f:
        return json.load(f)


def load_model_args(path: str) -> dict:
    """Load model_args.json from a model directory or file path.

    Args:
        path: Path to model_args.json or the directory containing it.

    Returns:
        Parsed model arguments dictionary.
    """
    if os.path.isdir(path):
        path = os.path.join(path, "model_args.json")
    with open(path) as f:
        return json.load(f)


def validate_config(
    config: dict,
    model_args: dict | None = None,
) -> list[str]:
    """Validate a config dict against FalconOCRConfig expectations.

    Returns a list of discrepancy descriptions. Empty list means valid.

    Args:
        config: The parsed config.json dictionary.
        model_args: Optional parsed model_args.json dictionary.

    Returns:
        List of discrepancy strings (empty = all valid).
    """
    issues: list[str] = []

    # Check canonical fields
    for field_name, expected_type, expected_value in _CANONICAL_FIELDS:
        if field_name not in config:
            issues.append(f"missing field: {field_name}")
            continue

        value = config[field_name]

        if not isinstance(value, expected_type):
            issues.append(
                f"{field_name}: expected type {expected_type.__name__ if isinstance(expected_type, type) else expected_type}, "
                f"got {type(value).__name__} ({value!r})"
            )
            continue

        if expected_value is not None:
            # For floats, use approximate comparison
            if isinstance(expected_value, float):
                if abs(value - expected_value) > expected_value * 1e-6:
                    issues.append(
                        f"{field_name}: expected {expected_value}, got {value}"
                    )
            else:
                if value != expected_value:
                    issues.append(
                        f"{field_name}: expected {expected_value}, got {value}"
                    )

    # Validate derived constraints
    dim = config.get("dim", 0)
    n_heads = config.get("n_heads", 0)
    head_dim = config.get("head_dim", 0)
    n_kv_heads = config.get("n_kv_heads", 0)
    ffn_dim = config.get("ffn_dim", 0)

    if n_heads > 0 and head_dim > 0:
        expected_qkv_dim = (n_heads + 2 * n_kv_heads) * head_dim
        # wqkv weight shape should be [expected_qkv_dim, dim]
        if expected_qkv_dim != 2048:
            issues.append(
                f"qkv dimension mismatch: (n_heads={n_heads} + 2*n_kv_heads={n_kv_heads}) * head_dim={head_dim} "
                f"= {expected_qkv_dim}, expected 2048"
            )

    if n_heads > 0 and n_kv_heads > 0:
        if n_heads % n_kv_heads != 0:
            issues.append(
                f"n_heads ({n_heads}) must be divisible by n_kv_heads ({n_kv_heads})"
            )

    if ffn_dim > 0 and dim > 0:
        # w13 is gated: ffn_dim * 2 interleaved, so w13 shape = [ffn_dim*2, dim]
        expected_w13_rows = ffn_dim * 2
        if expected_w13_rows != 4608:
            issues.append(
                f"w13 row count mismatch: ffn_dim*2 = {expected_w13_rows}, expected 4608"
            )

    # Cross-validate with model_args if provided
    if model_args is not None:
        shared_fields = [
            "dim",
            "n_layers",
            "n_heads",
            "head_dim",
            "n_kv_heads",
            "ffn_dim",
            "vocab_size",
            "max_seq_len",
            "rope_theta",
            "norm_eps",
            "channel_size",
            "spatial_patch_size",
            "temporal_patch_size",
            "eos_id",
            "img_id",
            "img_end_id",
            "image_cls_token_id",
            "image_reg_1_token_id",
            "image_reg_2_token_id",
            "image_reg_3_token_id",
            "image_reg_4_token_id",
        ]
        for field in shared_fields:
            if field in config and field in model_args:
                if config[field] != model_args[field]:
                    issues.append(
                        f"config vs model_args mismatch on {field}: "
                        f"{config[field]} vs {model_args[field]}"
                    )

        # model_args has extra fields not in config.json
        extra_model_args = set(model_args.keys()) - set(config.keys())
        expected_extras = {
            "coord_dec_dim",
            "coord_enc_dim",
            "coord_out_dim",
            "coord_token_id",
            "img_row_sep_id",
            "img_start_id",
            "num_segm_layers",
            "perception_heads",
            "seg_token_id",
            "segm_out_dim",
            "size_dec_dim",
            "size_enc_dim",
            "size_out_dim",
            "size_token_id",
        }
        unexpected_extras = extra_model_args - expected_extras
        if unexpected_extras:
            issues.append(
                f"unexpected extra model_args fields: {sorted(unexpected_extras)}"
            )

    return issues


def summary(config: dict) -> str:
    """Return a human-readable summary of the model configuration."""
    dim = config.get("dim", "?")
    n_layers = config.get("n_layers", "?")
    n_heads = config.get("n_heads", "?")
    n_kv_heads = config.get("n_kv_heads", "?")
    vocab_size = config.get("vocab_size", "?")
    max_seq_len = config.get("max_seq_len", "?")
    ffn_dim = config.get("ffn_dim", "?")
    patch_size = config.get("spatial_patch_size", "?")

    # Estimate parameter count
    params = 0
    if isinstance(dim, int) and isinstance(n_layers, int):
        if isinstance(vocab_size, int):
            params += vocab_size * dim * 2  # tok_embeddings + output
        params += dim * dim  # img_projector
        params += dim  # norm
        if isinstance(n_heads, int) and isinstance(n_kv_heads, int):
            head_dim = config.get("head_dim", 64)
            qkv_dim = (n_heads + 2 * n_kv_heads) * head_dim
            wo_dim = n_heads * head_dim
            params += n_layers * (
                qkv_dim * dim  # wqkv
                + dim * wo_dim  # wo
                + n_heads  # sinks
            )
        if isinstance(ffn_dim, int):
            params += n_layers * (
                ffn_dim * 2 * dim  # w13
                + dim * ffn_dim  # w2
            )

    param_str = f"{params / 1e6:.1f}M" if params > 0 else "?"

    return (
        f"Falcon-OCR: dim={dim}, layers={n_layers}, heads={n_heads}/{n_kv_heads} (Q/KV), "
        f"ffn={ffn_dim}, vocab={vocab_size}, max_seq={max_seq_len}, "
        f"patch={patch_size}x{patch_size}, params~{param_str}"
    )
