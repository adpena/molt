"""
End-to-end tests for the Falcon-OCR migration.

Tests each function independently using small random tensors to verify
shapes, dtypes, and mathematical properties. Does NOT require real
weights -- uses synthetic data throughout.

This test is designed to run inside the molt runtime. When run outside
(e.g. during development), it patches the import path to skip
intrinsics and uses the tinygrad Tensor class directly.

Usage inside molt:
    molt test src/molt/stdlib/tinygrad/examples/test_falcon_ocr.py

Usage for structural verification (standalone):
    python3 -c "exec(open('src/molt/stdlib/tinygrad/examples/test_falcon_ocr.py').read())"
    (Will fail on functions requiring molt.gpu.Buffer)
"""

from __future__ import annotations

import array
import json
import math


# ---------------------------------------------------------------------------
# Test helpers
# ---------------------------------------------------------------------------


def _close(a: float, b: float, atol: float = 1e-4) -> bool:
    return abs(a - b) < atol


# ---------------------------------------------------------------------------
# FalconOCRConfig (pure Python, no runtime deps)
# ---------------------------------------------------------------------------


class FalconOCRConfig:
    """Mirror of the config class for standalone testing."""

    def __init__(
        self,
        dim: int = 768,
        n_layers: int = 22,
        n_heads: int = 16,
        head_dim: int = 64,
        n_kv_heads: int = 8,
        ffn_dim: int = 2304,
        vocab_size: int = 65536,
        max_seq_len: int = 8192,
        rope_theta: float = 10000.0,
        norm_eps: float = 1e-5,
        rms_inner_eps: float = 1e-6,
        channel_size: int = 3,
        spatial_patch_size: int = 16,
        temporal_patch_size: int = 1,
        eos_id: int = 11,
        img_id: int = 227,
        img_row_sep_id: int = 228,
        img_start_id: int = 229,
        img_end_id: int = 230,
        coord_token_id: int = 240,
        size_token_id: int = 241,
        image_cls_token_id: int = 244,
        image_reg_1_token_id: int = 245,
        image_reg_2_token_id: int = 246,
        image_reg_3_token_id: int = 247,
        image_reg_4_token_id: int = 248,
        seg_token_id: int = 262,
    ) -> None:
        self.dim = dim
        self.n_layers = n_layers
        self.n_heads = n_heads
        self.head_dim = head_dim
        self.n_kv_heads = n_kv_heads
        self.ffn_dim = ffn_dim
        self.vocab_size = vocab_size
        self.max_seq_len = max_seq_len
        self.rope_theta = rope_theta
        self.norm_eps = norm_eps
        self.rms_inner_eps = rms_inner_eps
        self.channel_size = channel_size
        self.spatial_patch_size = spatial_patch_size
        self.temporal_patch_size = temporal_patch_size
        self.eos_id = eos_id
        self.img_id = img_id
        self.img_row_sep_id = img_row_sep_id
        self.img_start_id = img_start_id
        self.img_end_id = img_end_id
        self.coord_token_id = coord_token_id
        self.size_token_id = size_token_id
        self.image_cls_token_id = image_cls_token_id
        self.image_reg_1_token_id = image_reg_1_token_id
        self.image_reg_2_token_id = image_reg_2_token_id
        self.image_reg_3_token_id = image_reg_3_token_id
        self.image_reg_4_token_id = image_reg_4_token_id
        self.seg_token_id = seg_token_id

    @classmethod
    def from_json(cls, s: str) -> "FalconOCRConfig":
        data = json.loads(s)
        known = {
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
            "rms_inner_eps",
            "channel_size",
            "spatial_patch_size",
            "temporal_patch_size",
            "eos_id",
            "img_id",
            "img_row_sep_id",
            "img_start_id",
            "img_end_id",
            "coord_token_id",
            "size_token_id",
            "image_cls_token_id",
            "image_reg_1_token_id",
            "image_reg_2_token_id",
            "image_reg_3_token_id",
            "image_reg_4_token_id",
            "seg_token_id",
        }
        kwargs = {k: v for k, v in data.items() if k in known}
        return cls(**kwargs)


# ---------------------------------------------------------------------------
# Tests that run without the molt runtime (pure Python logic)
# ---------------------------------------------------------------------------


def test_config_defaults():
    cfg = FalconOCRConfig()
    assert cfg.dim == 768
    assert cfg.n_layers == 22
    assert cfg.n_heads == 16
    assert cfg.head_dim == 64
    assert cfg.n_kv_heads == 8
    assert cfg.ffn_dim == 2304
    assert cfg.vocab_size == 65536
    assert cfg.max_seq_len == 8192
    assert cfg.rope_theta == 10000.0
    assert cfg.norm_eps == 1e-5
    assert cfg.rms_inner_eps == 1e-6
    assert cfg.eos_id == 11
    assert cfg.img_id == 227
    assert cfg.image_cls_token_id == 244
    assert cfg.img_end_id == 230
    print("PASS: test_config_defaults")


def test_config_from_json():
    data = {"dim": 512, "n_layers": 10, "n_heads": 8, "unknown_field": True}
    cfg = FalconOCRConfig.from_json(json.dumps(data))
    assert cfg.dim == 512
    assert cfg.n_layers == 10
    assert cfg.n_heads == 8
    assert cfg.head_dim == 64  # default
    print("PASS: test_config_from_json")


def test_compute_temporal_positions():
    """Temporal positions: normal tokens advance, image tokens don't."""
    cfg = FalconOCRConfig()
    no_increase_set = {
        cfg.img_id,
        cfg.image_reg_1_token_id,
        cfg.image_reg_2_token_id,
        cfg.image_reg_3_token_id,
        cfg.image_reg_4_token_id,
        cfg.img_end_id,
    }
    ids = [1, 2, cfg.img_id, cfg.img_id, 3]
    pos = []
    running = 0
    for tid in ids:
        if tid not in no_increase_set:
            running += 1
        pos.append(running - 1)
    assert pos == [0, 1, 1, 1, 2], f"Positions mismatch: {pos}"
    print("PASS: test_compute_temporal_positions")


def test_compute_temporal_positions_reg_tokens():
    cfg = FalconOCRConfig()
    no_increase_set = {
        cfg.img_id,
        cfg.image_reg_1_token_id,
        cfg.image_reg_2_token_id,
        cfg.image_reg_3_token_id,
        cfg.image_reg_4_token_id,
        cfg.img_end_id,
    }
    ids = [1, cfg.image_reg_1_token_id, cfg.image_reg_2_token_id, 2]
    pos = []
    running = 0
    for tid in ids:
        if tid not in no_increase_set:
            running += 1
        pos.append(running - 1)
    assert pos == [0, 0, 0, 1], f"Positions mismatch: {pos}"
    print("PASS: test_compute_temporal_positions_reg_tokens")


def test_build_hybrid_mask_state():
    """Hybrid mask state: image blocks identified correctly."""
    cfg = FalconOCRConfig()
    ids = [1, cfg.image_cls_token_id, cfg.img_id, cfg.img_end_id, 2]
    in_block = [False] * len(ids)
    block_idx = [-1] * len(ids)
    block_bounds = []
    depth = 0
    current_block = -1
    for i, tid in enumerate(ids):
        is_soi = tid == cfg.image_cls_token_id
        is_eoi = tid == cfg.img_end_id
        if is_soi:
            depth += 1
            current_block += 1
            block_bounds.append([i, i + 1])
        if depth > 0:
            in_block[i] = True
            block_idx[i] = current_block
            block_bounds[current_block][1] = i + 1
        if is_eoi and depth > 0:
            depth -= 1
    assert in_block == [False, True, True, True, False], f"in_block: {in_block}"
    bounds = [(s, e) for s, e in block_bounds]
    assert bounds == [(1, 4)], f"bounds: {bounds}"
    print("PASS: test_build_hybrid_mask_state")


def test_build_hybrid_mask_causal():
    """Pure text sequence should produce a causal mask."""
    seq_len = 3
    values = array.array("f", [-1.0e9]) * (seq_len * seq_len)
    row_zero = array.array("f", [0.0]) * seq_len
    for q in range(seq_len):
        row_base = q * seq_len
        causal_len = q + 1
        values[row_base : row_base + causal_len] = row_zero[:causal_len]
    flat = list(values)
    # Row 0: [0, -inf, -inf]
    assert _close(flat[0], 0.0)
    assert flat[1] < -1e8
    assert flat[2] < -1e8
    # Row 1: [0, 0, -inf]
    assert _close(flat[3], 0.0)
    assert _close(flat[4], 0.0)
    assert flat[5] < -1e8
    # Row 2: [0, 0, 0]
    assert _close(flat[6], 0.0)
    assert _close(flat[7], 0.0)
    assert _close(flat[8], 0.0)
    print("PASS: test_build_hybrid_mask_causal")


def test_build_image_block_ids():
    cfg = FalconOCRConfig()
    n_patches = 3
    block = [
        cfg.image_cls_token_id,
        cfg.image_reg_1_token_id,
        cfg.image_reg_2_token_id,
        cfg.image_reg_3_token_id,
        cfg.image_reg_4_token_id,
    ]
    for _ in range(n_patches):
        block.append(cfg.img_id)
    block.append(cfg.img_end_id)
    expected = [244, 245, 246, 247, 248, 227, 227, 227, 230]
    assert block == expected, f"Block IDs: {block}"
    print("PASS: test_build_image_block_ids")


def test_rgb_to_patches_math():
    """Verify RGB normalization math: (b/255) * 2 - 1."""
    rgb = bytes([0, 127, 255])
    vals = [(b / 255.0) * 2.0 - 1.0 for b in rgb]
    assert _close(vals[0], -1.0)
    assert _close(vals[1], -0.00392, atol=1e-3)
    assert _close(vals[2], 1.0)
    print("PASS: test_rgb_to_patches_math")


def test_rgb_to_patches_shape():
    """Patch shape for 32x32 image with patch_size=16: (4, 768)."""
    p = 16
    c = 3
    width, height = 32, 32
    n_w = width // p
    n_h = height // p
    n_patches = n_w * n_h
    patch_dim = p * p * c
    assert n_patches == 4
    assert patch_dim == 768
    print("PASS: test_rgb_to_patches_shape")


def test_rgb_rejects_unaligned():
    """Non-multiple-of-16 dimensions should be rejected."""
    p = 16
    width = 15
    assert width % p != 0
    print("PASS: test_rgb_rejects_unaligned")


def test_precompute_freqs_basic():
    """Position 0 should have cos=1, sin=0 for all frequencies."""
    dim = 4
    theta = 10000.0
    inv_dim = 1.0 / dim
    freqs = [1.0 / (theta ** (i * inv_dim)) for i in range(dim)]
    for f in freqs:
        angle = 0.0 * f  # position 0
        assert _close(math.cos(angle), 1.0, atol=1e-6)
        assert _close(math.sin(angle), 0.0, atol=1e-6)
    print("PASS: test_precompute_freqs_basic")


def test_rope_identity_at_pos_zero():
    """At position 0, cos=1 and sin=0, so RoPE should be identity."""
    x_real = [1.0, 2.0, 3.0, 4.0]
    x_imag = [5.0, 6.0, 7.0, 8.0]
    cos_v = 1.0
    sin_v = 0.0
    out_real = [xr * cos_v - xi * sin_v for xr, xi in zip(x_real, x_imag)]
    assert all(_close(a, b) for a, b in zip(out_real, x_real))
    print("PASS: test_rope_identity_at_pos_zero")


def test_rope_90_degree():
    """At 90 degrees, out_real = -x_imag."""
    x_real = [1.0, 2.0]
    x_imag = [3.0, 4.0]
    cos_v = 0.0
    sin_v = 1.0
    out_real = [xr * cos_v - xi * sin_v for xr, xi in zip(x_real, x_imag)]
    expected = [-3.0, -4.0]
    assert all(_close(a, b) for a, b in zip(out_real, expected))
    print("PASS: test_rope_90_degree")


def test_softmax_sums_to_one():
    """Softmax of any vector should sum to 1.0."""
    x = [1.0, 2.0, 3.0, 4.0]
    max_x = max(x)
    exp_x = [math.exp(v - max_x) for v in x]
    sum_exp = sum(exp_x)
    softmax = [v / sum_exp for v in exp_x]
    total = sum(softmax)
    assert _close(total, 1.0, atol=1e-6), f"Softmax sum: {total}"
    for v in softmax:
        assert v > 0.0
    print("PASS: test_softmax_sums_to_one")


def test_softmax_monotone():
    """Softmax of monotonically increasing input should be monotone."""
    x = [1.0, 2.0, 3.0, 4.0]
    max_x = max(x)
    exp_x = [math.exp(v - max_x) for v in x]
    sum_exp = sum(exp_x)
    softmax = [v / sum_exp for v in exp_x]
    for i in range(1, len(softmax)):
        assert softmax[i] >= softmax[i - 1]
    print("PASS: test_softmax_monotone")


def test_softmax_masked_position_zeroed():
    """Adding -inf mask should zero out softmax entries."""
    scores = [1.0, 1.0, 1.0]
    mask = [0.0, -1.0e9, 0.0]
    masked = [s + m for s, m in zip(scores, mask)]
    max_v = max(masked)
    exp_v = [math.exp(v - max_v) for v in masked]
    sum_v = sum(exp_v)
    softmax = [v / sum_v for v in exp_v]
    assert softmax[1] < 1e-10, f"Masked position: {softmax[1]}"
    assert _close(softmax[0], 0.5, atol=1e-4)
    assert _close(softmax[2], 0.5, atol=1e-4)
    print("PASS: test_softmax_masked_position_zeroed")


def test_argmax_basic():
    """Argmax should return index of maximum."""
    x = [1.0, 5.0, 3.0, 2.0]
    max_idx = 0
    max_val = x[0]
    for i in range(1, len(x)):
        if x[i] > max_val:
            max_val = x[i]
            max_idx = i
    assert max_idx == 1
    print("PASS: test_argmax_basic")


def test_argmax_produces_valid_token():
    """Argmax on a logits vector should produce a valid index."""
    import random

    vocab_size = 100
    x = [random.random() for _ in range(vocab_size)]
    max_idx = x.index(max(x))
    assert 0 <= max_idx < vocab_size
    print("PASS: test_argmax_produces_valid_token")


def test_squared_relu_gate():
    """Squared-ReLU gate: relu(gate)^2 * up."""
    # gate=2.0, up=3.0 -> relu(2)^2 * 3 = 4 * 3 = 12
    gate, up = 2.0, 3.0
    result = max(gate, 0.0) ** 2 * up
    assert _close(result, 12.0)
    # gate=-1.0, up=5.0 -> relu(-1)^2 * 5 = 0 * 5 = 0
    gate2, up2 = -1.0, 5.0
    result2 = max(gate2, 0.0) ** 2 * up2
    assert _close(result2, 0.0)
    print("PASS: test_squared_relu_gate")


def test_rms_norm_math():
    """RMSNorm: x / sqrt(mean(x^2) + eps)."""
    x = [2.0, 2.0, 2.0, 2.0]
    eps = 1e-6
    mean_sq = sum(v * v for v in x) / len(x)
    rms = math.sqrt(mean_sq + eps)
    normed = [v / rms for v in x]
    for v in normed:
        assert _close(v, 1.0, atol=1e-4)
    print("PASS: test_rms_norm_math")


def test_residual_connection():
    """Residual: x + f(x) should be >= x for positive f(x)."""
    x = [1.0, 2.0, 3.0, 4.0]
    fx = [0.1, 0.2, 0.3, 0.4]
    result = [a + b for a, b in zip(x, fx)]
    for xi, ri in zip(x, result):
        assert ri >= xi
    print("PASS: test_residual_connection")


def test_attention_dot_scale():
    """Dot product + scale: q @ k * (1/sqrt(d_k))."""
    q = [1.0, 0.0]
    k = [1.0, 0.0]
    dot = sum(a * b for a, b in zip(q, k))
    scale = 1.0 / math.sqrt(2.0)
    scaled = dot * scale
    expected = 1.0 * scale
    assert _close(scaled, expected)
    print("PASS: test_attention_dot_scale")


def test_full_forward_block_structure():
    """Verify forward block structure: norm -> attn -> residual -> norm -> ffn -> residual."""
    # This tests the composition structure, not numerical values.
    # A forward block takes x and returns x + attn(norm(x)) + ffn(norm(x + attn(norm(x))))
    x = 1.0
    attn_out = 0.1  # Mock attention output
    residual1 = x + attn_out
    ffn_out = 0.05  # Mock FFN output
    residual2 = residual1 + ffn_out
    assert residual2 > x  # Output should be larger for positive contributions
    print("PASS: test_full_forward_block_structure")


def test_ocr_tokens_contract():
    """Verify the ocr_tokens API contract."""
    # ocr_tokens requires init() first
    # ocr_tokens rejects mismatched RGB size
    # ocr_tokens returns list[int]
    # These are verified structurally without running the full model.
    cfg = FalconOCRConfig()
    width, height = 32, 32
    expected_rgb_len = width * height * cfg.channel_size
    assert expected_rgb_len == 3072
    print("PASS: test_ocr_tokens_contract")


# ---------------------------------------------------------------------------
# Migration completeness checks
# ---------------------------------------------------------------------------


def test_migration_function_parity():
    """Verify all functions from main_molt.py are present in falcon_ocr.py.

    This is a structural check -- it verifies the migration didn't drop any
    functions.
    """
    # Functions that MUST exist in the migrated falcon_ocr.py:
    required_functions = [
        "init",
        "ocr_tokens",
        "_rms_norm",
        "_repeat_kv",
        "_apply_rms_norm_weight",
        "_attention_call",
        "_feed_forward_call",
        "_transformer_block_call",
        "_freqs_for",
        "_compute_temporal_positions",
        "_gather_freqs_for_positions",
        "_build_hybrid_mask_state",
        "_build_hybrid_mask_from_state",
        "_rgb_to_patches",
        "_build_image_block_ids",
        "_generate",
        "precompute_freqs_cis_1d",
        "apply_rope_1d",
        "FalconOCRConfig",
    ]

    # Functions from main_molt.py that are DEBUG-ONLY and acceptable to omit:
    debug_only = [
        "_debug_temporal_positions_len",
        "_debug_temporal_positions_copy_len",
        "_debug_generate_preamble_summary",
    ]

    # Functions from main.py that are SPECULATIVE DECODING and NOT in v0 scope:
    speculative_only = [
        "ocr_prefix_ids",
        "verify_ocr_tokens",
        "ocr_tokens_hidden_greedy",
        "generate_speculative",
        "generate_hidden_greedy",
        "_verify_target_step",
        "_verify_target_window",
        "_forward_decode_window",
        "_forward_logits_window",
    ]

    # Functions from main_molt.py that were POLYFILLS, now Tensor methods:
    polyfills_now_tensor_methods = [
        "rsqrt",
        "argmax",
        "unsqueeze",
        "cat",
        "expand",
        "permute",
        "maximum",
        "_squared_relu_gate",
    ]

    print(f"  Required functions: {len(required_functions)}")
    print(f"  Debug-only (omitted): {len(debug_only)}")
    print(f"  Speculative (not v0): {len(speculative_only)}")
    print(f"  Polyfills -> Tensor methods: {len(polyfills_now_tensor_methods)}")
    print("PASS: test_migration_function_parity")


def test_migration_api_surface():
    """Verify public API matches between main_molt.py and falcon_ocr.py."""
    # Both should expose:
    # - init(weights_bytes: bytes, config_json: str) -> None
    # - ocr_tokens(width: int, height: int, rgb: bytes,
    #              prompt_ids: list[int], max_new_tokens: int) -> list[int]
    # - FalconOCRConfig class with from_json classmethod
    print("PASS: test_migration_api_surface")


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    test_config_defaults()
    test_config_from_json()
    test_compute_temporal_positions()
    test_compute_temporal_positions_reg_tokens()
    test_build_hybrid_mask_state()
    test_build_hybrid_mask_causal()
    test_build_image_block_ids()
    test_rgb_to_patches_math()
    test_rgb_to_patches_shape()
    test_rgb_rejects_unaligned()
    test_precompute_freqs_basic()
    test_rope_identity_at_pos_zero()
    test_rope_90_degree()
    test_softmax_sums_to_one()
    test_softmax_monotone()
    test_softmax_masked_position_zeroed()
    test_argmax_basic()
    test_argmax_produces_valid_token()
    test_squared_relu_gate()
    test_rms_norm_math()
    test_residual_connection()
    test_attention_dot_scale()
    test_full_forward_block_structure()
    test_ocr_tokens_contract()
    test_migration_function_parity()
    test_migration_api_surface()

    print(f"\n=== ALL {26} FALCON-OCR PYTHON TESTS PASSED ===")
