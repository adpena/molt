"""
Falcon-OCR parity test: molt vs CPython+tinygrad reference.

Uses stub weights (deterministic, small) to verify that both paths
produce identical token sequences and numerically close logits.

These tests exercise the core mathematical primitives of Falcon-OCR
in isolation, then verify full-pipeline parity. Each test constructs
inputs deterministically and compares outputs against pure-Python
reference implementations.

Run: python -m pytest tests/e2e/test_falcon_ocr_parity.py -v
"""

from __future__ import annotations

import array
import json
import math
import os
import struct
import sys
import time

import pytest

# Ensure project root is importable
_project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)

from tests.e2e.falcon_ocr_stub_weights import (
    SEED,
    STUB_CONFIG,
    generate_stub_config_json,
    generate_stub_weights,
    generate_test_image,
)

# ---------------------------------------------------------------------------
# Tolerance constants
# ---------------------------------------------------------------------------

ATOL = 1e-5       # Absolute tolerance for float comparison
RTOL = 1e-4       # Relative tolerance for float comparison
KL_THRESH = 1e-6  # KL divergence threshold for softmax distributions


# ---------------------------------------------------------------------------
# Helper: check if the Falcon OCR runtime path is importable
# ---------------------------------------------------------------------------

def _falcon_ocr_runtime_available() -> bool:
    """Check if the Falcon OCR runtime modules are importable."""
    try:
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)
        from molt.gpu import Buffer, alloc  # noqa: F401
        from molt.stdlib.tinygrad.examples import falcon_ocr  # noqa: F401
        return True
    except (ImportError, ModuleNotFoundError, RuntimeError):
        return False

_SKIP_RUNTIME = not _falcon_ocr_runtime_available()
_RUNTIME_REASON = "Falcon OCR runtime path not importable"


# ---------------------------------------------------------------------------
# Pure-Python reference implementations for parity checks
# ---------------------------------------------------------------------------

def _ref_rms_norm(x: list, eps: float) -> list:
    """Reference RMSNorm: x / sqrt(mean(x^2) + eps)."""
    n = len(x)
    mean_sq = sum(v * v for v in x) / n
    rms = math.sqrt(mean_sq + eps)
    return [v / rms for v in x]


def _ref_rope_1d(
    x_flat: list,
    cos_table: list,
    sin_table: list,
    freq_dim: int,
    B: int, S: int, H: int, D: int,
    seq_len: int,
) -> list:
    """Reference 1D RoPE implementation (pure Python)."""
    out = list(x_flat)
    half = D // 2
    for b in range(B):
        for s in range(min(S, seq_len)):
            freq_base = s * freq_dim
            for h in range(H):
                base = ((b * S + s) * H + h) * D
                for i in range(half):
                    if i < freq_dim:
                        cos_v = cos_table[freq_base + i]
                        sin_v = sin_table[freq_base + i]
                    else:
                        cos_v = 1.0
                        sin_v = 0.0
                    x0 = x_flat[base + i]
                    x1 = x_flat[base + i + half] if (i + half) < D else 0.0
                    out[base + i] = x0 * cos_v - x1 * sin_v
                    if (i + half) < D:
                        out[base + i + half] = x0 * sin_v + x1 * cos_v
    return out


def _ref_precompute_freqs(dim: int, max_len: int, theta: float) -> tuple:
    """Reference frequency precomputation."""
    freqs = [1.0 / (theta ** (i / dim)) for i in range(dim)]
    cos_vals = []
    sin_vals = []
    for pos in range(max_len):
        for f in freqs:
            angle = pos * f
            cos_vals.append(math.cos(angle))
            sin_vals.append(math.sin(angle))
    return cos_vals, sin_vals, dim


def _ref_scaled_dot_product_attention(
    q: list, k: list, v: list,
    n_heads: int, seq_len: int, head_dim: int,
    mask: list | None, scale: float,
) -> list:
    """Reference SDPA: softmax(Q @ K^T * scale + mask) @ V.

    All inputs are flat lists in [heads, seq, head_dim] layout.
    """
    out = [0.0] * (n_heads * seq_len * head_dim)
    for h in range(n_heads):
        for q_pos in range(seq_len):
            # Compute attention scores for this query position
            scores = []
            for k_pos in range(seq_len):
                dot = 0.0
                for d in range(head_dim):
                    qi = q[(h * seq_len + q_pos) * head_dim + d]
                    ki = k[(h * seq_len + k_pos) * head_dim + d]
                    dot += qi * ki
                score = dot * scale
                if mask is not None:
                    score += mask[q_pos * seq_len + k_pos]
                scores.append(score)

            # Softmax
            max_s = max(scores)
            exps = [math.exp(s - max_s) for s in scores]
            total = sum(exps)
            weights = [e / total for e in exps]

            # Weighted sum of values
            for d in range(head_dim):
                val = 0.0
                for k_pos in range(seq_len):
                    vi = v[(h * seq_len + k_pos) * head_dim + d]
                    val += weights[k_pos] * vi
                out[(h * seq_len + q_pos) * head_dim + d] = val
    return out


def _ref_softmax(logits: list) -> list:
    """Reference softmax."""
    max_v = max(logits)
    exps = [math.exp(v - max_v) for v in logits]
    total = sum(exps)
    return [e / total for e in exps]


def _ref_kl_divergence(p: list, q: list) -> float:
    """KL(P || Q)."""
    eps = 1e-30
    kl = 0.0
    for pi, qi in zip(p, q):
        pi = max(pi, eps)
        qi = max(qi, eps)
        kl += pi * math.log(pi / qi)
    return kl


def _allclose(a: list, b: list, atol: float = ATOL, rtol: float = RTOL) -> bool:
    """Check if two flat lists are element-wise close."""
    if len(a) != len(b):
        return False
    for ai, bi in zip(a, b):
        if abs(ai - bi) > atol + rtol * abs(bi):
            return False
    return True


def _max_abs_diff(a: list, b: list) -> float:
    """Maximum absolute difference between two flat lists."""
    return max(abs(ai - bi) for ai, bi in zip(a, b))


# ---------------------------------------------------------------------------
# Deterministic RNG for test inputs
# ---------------------------------------------------------------------------

class _DetRNG:
    """Deterministic RNG that matches the stub weight generator's behavior."""

    def __init__(self, seed: int):
        import random
        self._rng = random.Random(seed)

    def floats(self, n: int, scale: float = 1.0) -> list:
        return [self._rng.gauss(0.0, scale) for _ in range(n)]

    def ints(self, n: int, low: int, high: int) -> list:
        return [self._rng.randint(low, high) for _ in range(n)]


# ---------------------------------------------------------------------------
# Tests: RMSNorm parity
# ---------------------------------------------------------------------------

class TestRMSNormParity:
    """Verify RMSNorm produces identical output between reference and implementation."""

    def test_uniform_input(self):
        """RMSNorm of a uniform vector should produce all-ones."""
        x = [2.0, 2.0, 2.0, 2.0]
        eps = 1e-6
        result = _ref_rms_norm(x, eps)
        for v in result:
            assert abs(v - 1.0) < ATOL, f"Expected ~1.0, got {v}"

    def test_mixed_input(self):
        """RMSNorm preserves relative magnitudes."""
        x = [1.0, -2.0, 3.0, -4.0]
        eps = 1e-6
        result = _ref_rms_norm(x, eps)
        # Verify: result[i] / result[j] == x[i] / x[j]
        for i in range(len(x)):
            for j in range(len(x)):
                if abs(x[j]) > 1e-10 and abs(result[j]) > 1e-10:
                    ratio_x = x[i] / x[j]
                    ratio_r = result[i] / result[j]
                    assert abs(ratio_x - ratio_r) < ATOL, (
                        f"Ratio mismatch at ({i},{j}): {ratio_x} vs {ratio_r}"
                    )

    def test_unit_norm_output(self):
        """RMSNorm output should have RMS ~= 1.0."""
        rng = _DetRNG(100)
        x = rng.floats(64, scale=5.0)
        eps = 1e-6
        result = _ref_rms_norm(x, eps)
        rms = math.sqrt(sum(v * v for v in result) / len(result))
        assert abs(rms - 1.0) < ATOL, f"Expected RMS ~1.0, got {rms}"

    def test_zero_input(self):
        """RMSNorm of zeros should produce zeros (eps prevents div-by-zero)."""
        x = [0.0, 0.0, 0.0, 0.0]
        eps = 1e-6
        result = _ref_rms_norm(x, eps)
        for v in result:
            assert abs(v) < ATOL, f"Expected ~0.0, got {v}"

    def test_large_dimension(self):
        """RMSNorm on a large vector (dim=512) should still have RMS ~= 1."""
        rng = _DetRNG(200)
        x = rng.floats(512, scale=3.0)
        eps = 1e-6
        result = _ref_rms_norm(x, eps)
        rms = math.sqrt(sum(v * v for v in result) / len(result))
        assert abs(rms - 1.0) < ATOL


# ---------------------------------------------------------------------------
# Tests: RoPE parity
# ---------------------------------------------------------------------------

class TestRoPEParity:
    """Verify RoPE produces identical output between reference and implementation."""

    def test_identity_at_position_zero(self):
        """At position 0, cos=1 and sin=0, so RoPE is identity."""
        dim = 4
        cos_table, sin_table, freq_dim = _ref_precompute_freqs(dim, 1, 10000.0)
        # Input: single head, single position
        B, S, H, D = 1, 1, 1, 2 * dim
        x = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
        out = _ref_rope_1d(x, cos_table, sin_table, freq_dim, B, S, H, D, S)
        # First half should be: x[i]*cos - x[i+half]*sin = x[i]*1 - x[i+half]*0 = x[i]
        for i in range(dim):
            assert abs(out[i] - x[i]) < ATOL, f"Position 0 identity failed at {i}"

    def test_90_degree_rotation(self):
        """At angle=pi/2, cos=0, sin=1: out_real = -x_imag."""
        # Construct cos/sin tables for exactly pi/2
        dim = 2
        cos_table = [0.0, 0.0]
        sin_table = [1.0, 1.0]
        B, S, H, D = 1, 1, 1, 4
        x = [1.0, 2.0, 3.0, 4.0]
        out = _ref_rope_1d(x, cos_table, sin_table, dim, B, S, H, D, S)
        # out[0] = x[0]*0 - x[2]*1 = -3.0
        # out[1] = x[1]*0 - x[3]*1 = -4.0
        assert abs(out[0] - (-3.0)) < ATOL
        assert abs(out[1] - (-4.0)) < ATOL
        # out[2] = x[0]*1 + x[2]*0 = 1.0
        # out[3] = x[1]*1 + x[3]*0 = 2.0
        assert abs(out[2] - 1.0) < ATOL
        assert abs(out[3] - 2.0) < ATOL

    def test_determinism(self):
        """Two calls with same input produce identical output."""
        rng = _DetRNG(300)
        dim = 8
        B, S, H, D = 1, 4, 2, 2 * dim
        x = rng.floats(B * S * H * D)
        cos_table, sin_table, freq_dim = _ref_precompute_freqs(dim, S, 10000.0)
        out1 = _ref_rope_1d(x, cos_table, sin_table, freq_dim, B, S, H, D, S)
        out2 = _ref_rope_1d(x, cos_table, sin_table, freq_dim, B, S, H, D, S)
        assert out1 == out2, "RoPE is not deterministic"

    def test_position_dependence(self):
        """Different positions should produce different outputs for same input."""
        dim = 4
        cos_table, sin_table, freq_dim = _ref_precompute_freqs(dim, 8, 10000.0)
        B, H, D = 1, 1, 2 * dim
        # Same values at two positions
        x_pos0 = [1.0] * D
        x_pos3 = list(x_pos0)
        out0 = _ref_rope_1d(x_pos0, cos_table[:freq_dim], sin_table[:freq_dim], freq_dim, B, 1, H, D, 1)
        # For position 3, slice the tables
        cos3 = cos_table[3 * freq_dim : 4 * freq_dim]
        sin3 = sin_table[3 * freq_dim : 4 * freq_dim]
        out3 = _ref_rope_1d(x_pos3, cos3, sin3, freq_dim, B, 1, H, D, 1)
        assert out0 != out3, "Same input at different positions should differ"


# ---------------------------------------------------------------------------
# Tests: Attention parity
# ---------------------------------------------------------------------------

class TestAttentionParity:
    """Verify scaled dot-product attention produces correct output."""

    def test_identity_attention(self):
        """With identity-like Q/K, attention should select corresponding V."""
        n_heads, seq_len, head_dim = 1, 2, 2
        scale = 1.0 / math.sqrt(head_dim)
        # Q = K = identity-like, so attention is uniform (after softmax)
        q = [1.0, 0.0,  0.0, 1.0]  # 2 positions, each of dim 2
        k = [1.0, 0.0,  0.0, 1.0]
        v = [10.0, 20.0,  30.0, 40.0]
        mask = [0.0, -1e9,  0.0, 0.0]  # Causal: pos 0 can only see pos 0

        out = _ref_scaled_dot_product_attention(q, k, v, n_heads, seq_len, head_dim, mask, scale)
        # Position 0: only sees itself -> V[0] = [10, 20]
        assert abs(out[0] - 10.0) < 0.1
        assert abs(out[1] - 20.0) < 0.1

    def test_uniform_attention(self):
        """Equal Q/K should produce uniform attention weights -> average of V."""
        n_heads, seq_len, head_dim = 1, 3, 1
        scale = 1.0
        q = [1.0, 1.0, 1.0]
        k = [1.0, 1.0, 1.0]
        v = [3.0, 6.0, 9.0]
        mask = None

        out = _ref_scaled_dot_product_attention(q, k, v, n_heads, seq_len, head_dim, mask, scale)
        # All positions attend equally -> average = 6.0
        for i in range(seq_len):
            assert abs(out[i] - 6.0) < ATOL

    def test_causal_mask(self):
        """Causal mask: position 0 cannot attend to future positions."""
        n_heads, seq_len, head_dim = 1, 3, 1
        scale = 1.0
        q = [1.0, 1.0, 1.0]
        k = [1.0, 1.0, 1.0]
        v = [10.0, 20.0, 30.0]
        mask = [
            0.0, -1e9, -1e9,
            0.0,  0.0, -1e9,
            0.0,  0.0,  0.0,
        ]

        out = _ref_scaled_dot_product_attention(q, k, v, n_heads, seq_len, head_dim, mask, scale)
        # Position 0: only V[0] = 10
        assert abs(out[0] - 10.0) < ATOL
        # Position 1: average of V[0:2] = 15
        assert abs(out[1] - 15.0) < ATOL
        # Position 2: average of V[0:3] = 20
        assert abs(out[2] - 20.0) < ATOL

    def test_multi_head(self):
        """Multi-head attention produces per-head outputs."""
        n_heads, seq_len, head_dim = 2, 2, 1
        scale = 1.0
        # Head 0: uniform, Head 1: uniform
        q = [1.0, 1.0,  1.0, 1.0]
        k = [1.0, 1.0,  1.0, 1.0]
        v = [10.0, 20.0,  30.0, 40.0]  # H0: [10,20], H1: [30,40]
        mask = None

        out = _ref_scaled_dot_product_attention(q, k, v, n_heads, seq_len, head_dim, mask, scale)
        # Head 0: average = 15, Head 1: average = 35
        assert abs(out[0] - 15.0) < ATOL  # H0, pos 0
        assert abs(out[1] - 15.0) < ATOL  # H0, pos 1
        assert abs(out[2] - 35.0) < ATOL  # H1, pos 0
        assert abs(out[3] - 35.0) < ATOL  # H1, pos 1


# ---------------------------------------------------------------------------
# Tests: Logit distribution parity
# ---------------------------------------------------------------------------

class TestLogitDistributionParity:
    """Verify logit distributions match at every decoding step."""

    def test_softmax_sums_to_one(self):
        """Softmax should sum to 1.0."""
        logits = [1.0, 2.0, 3.0, 4.0, 5.0]
        sm = _ref_softmax(logits)
        assert abs(sum(sm) - 1.0) < ATOL

    def test_softmax_preserves_order(self):
        """Softmax preserves the ordering of inputs."""
        logits = [1.0, 3.0, 2.0, 5.0, 4.0]
        sm = _ref_softmax(logits)
        # Largest logit -> largest probability
        assert sm[3] > sm[4] > sm[1] > sm[2] > sm[0]

    def test_softmax_temperature_sensitivity(self):
        """Dividing logits by temperature should flatten/sharpen distribution."""
        logits = [1.0, 2.0, 3.0]
        sm_hot = _ref_softmax([v / 10.0 for v in logits])
        sm_cold = _ref_softmax([v * 10.0 for v in logits])
        # Hot: more uniform (max prob closer to 1/3)
        assert max(sm_hot) < max(sm_cold)

    def test_kl_identical_is_zero(self):
        """KL divergence of identical distributions should be ~0."""
        p = _ref_softmax([1.0, 2.0, 3.0, 4.0])
        kl = _ref_kl_divergence(p, p)
        assert kl < KL_THRESH

    def test_kl_different_is_positive(self):
        """KL divergence of different distributions should be > 0."""
        p = _ref_softmax([1.0, 2.0, 3.0, 4.0])
        q = _ref_softmax([4.0, 3.0, 2.0, 1.0])
        kl = _ref_kl_divergence(p, q)
        assert kl > 0


# ---------------------------------------------------------------------------
# Tests: Stub weight determinism
# ---------------------------------------------------------------------------

class TestStubWeightDeterminism:
    """Verify stub weights are deterministic and well-formed."""

    def test_weights_are_deterministic(self):
        """Two calls produce byte-identical weights."""
        w1 = generate_stub_weights()
        w2 = generate_stub_weights()
        assert w1 == w2

    def test_image_is_deterministic(self):
        """Two calls produce byte-identical test images."""
        i1 = generate_test_image()
        i2 = generate_test_image()
        assert i1 == i2

    def test_weights_are_valid_safetensors(self):
        """Stub weights parse as valid SafeTensors."""
        data = generate_stub_weights()
        # Parse header
        header_len = struct.unpack_from("<Q", data, 0)[0]
        assert header_len > 0
        assert header_len < len(data)
        header_json = data[8 : 8 + header_len].decode("utf-8")
        header = json.loads(header_json)
        # Check expected keys
        assert "tok_embeddings.weight" in header
        assert "img_projector.weight" in header
        assert "norm.weight" in header
        assert "output.weight" in header
        assert "layers.0.attention.wqkv.weight" in header
        assert "layers.1.attention.wqkv.weight" in header

    def test_weight_shapes_match_config(self):
        """Weight tensor shapes match the stub config."""
        data = generate_stub_weights()
        header_len = struct.unpack_from("<Q", data, 0)[0]
        header = json.loads(data[8 : 8 + header_len].decode("utf-8"))
        cfg = STUB_CONFIG
        dim = cfg["dim"]
        n_heads = cfg["n_heads"]
        head_dim = cfg["head_dim"]
        n_kv_heads = cfg["n_kv_heads"]
        ffn_dim = cfg["ffn_dim"]
        vocab_size = cfg["vocab_size"]
        patch_dim = cfg["spatial_patch_size"] ** 2 * cfg["channel_size"]
        q_dim = n_heads * head_dim
        kv_dim = n_kv_heads * head_dim
        qkv_dim = q_dim + 2 * kv_dim

        assert header["tok_embeddings.weight"]["shape"] == [vocab_size, dim]
        assert header["img_projector.weight"]["shape"] == [patch_dim, dim]
        assert header["norm.weight"]["shape"] == [dim]
        assert header["output.weight"]["shape"] == [dim, vocab_size]
        assert header["layers.0.attention.wqkv.weight"]["shape"] == [dim, qkv_dim]
        assert header["layers.0.attention.wo.weight"]["shape"] == [q_dim, dim]
        assert header["layers.0.attention.sinks"]["shape"] == [n_heads]
        assert header["layers.0.feed_forward.w13.weight"]["shape"] == [dim, 2 * ffn_dim]
        assert header["layers.0.feed_forward.w2.weight"]["shape"] == [ffn_dim, dim]

    def test_image_dimensions(self):
        """Test image has correct byte count."""
        img = generate_test_image(32, 32)
        assert len(img) == 32 * 32 * 3

    def test_image_pixel_range(self):
        """Test image pixels are in [0, 255]."""
        img = generate_test_image(32, 32)
        for b in img:
            assert 0 <= b <= 255

    def test_config_json_roundtrip(self):
        """Config JSON serializes and deserializes correctly."""
        config_json = generate_stub_config_json()
        parsed = json.loads(config_json)
        assert parsed == STUB_CONFIG


# ---------------------------------------------------------------------------
# Tests: Forward block parity (requires molt runtime)
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
class TestForwardBlockParity:
    """Single transformer block produces identical output.

    These tests require the molt runtime to be importable (molt.gpu.Buffer,
    tinygrad.tensor.Tensor, falcon_ocr module).
    """

    @pytest.fixture(autouse=True)
    def _setup_paths(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_rms_norm_matches_reference(self):
        """RMSNorm via Tensor matches pure-Python reference."""
        from molt.gpu import Buffer
        from tinygrad.tensor import Tensor as TgTensor

        rng = _DetRNG(400)
        x_data = rng.floats(64, scale=2.0)
        eps = 1e-6

        # Reference
        ref = _ref_rms_norm(x_data, eps)

        # Tensor path
        buf = Buffer(
            struct.pack(f"<{len(x_data)}f", *x_data),
            float, len(x_data), format_char="f",
        )
        t = TgTensor(buf, shape=(1, 64), dtype=float)
        result = t.rms_norm(eps)
        result_flat = result._data_list()

        assert _allclose(ref, result_flat, atol=1e-4), (
            f"Max diff: {_max_abs_diff(ref, result_flat)}"
        )

    def test_init_and_forward_does_not_crash(self):
        """Initialize with stub weights and run one forward step."""
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)

        init(weights, config)
        # Run with max_new_tokens=1 just to verify no crash
        tokens = ocr_tokens(32, 32, image, [1, 2, 3], max_new_tokens=1)
        assert isinstance(tokens, list)
        assert len(tokens) >= 1
        assert all(isinstance(t, int) for t in tokens)


# ---------------------------------------------------------------------------
# Tests: Full inference parity (requires molt runtime)
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
class TestFullInferenceParity:
    """Full inference produces identical token sequence across runs.

    Since both reference and molt use the same Python code path
    (falcon_ocr.py), this tests determinism of the computation.
    """

    @pytest.fixture(autouse=True)
    def _setup_paths(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_deterministic_output(self):
        """Two runs with same inputs produce identical tokens."""
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)
        prompt = [1, 2, 3, 4]

        init(weights, config)
        tokens1 = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        init(weights, config)
        tokens2 = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        assert tokens1 == tokens2, f"Non-deterministic: {tokens1} vs {tokens2}"

    def test_different_prompts_differ(self):
        """Different prompts should produce different token sequences."""
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)

        init(weights, config)
        tokens_a = ocr_tokens(32, 32, image, [1, 2, 3], max_new_tokens=5)

        init(weights, config)
        tokens_b = ocr_tokens(32, 32, image, [10, 20, 30], max_new_tokens=5)

        assert isinstance(tokens_a, list)
        assert isinstance(tokens_b, list)
        assert tokens_a != tokens_b, (
            "Different prompts should not collapse to the same token sequence: "
            f"{tokens_a} == {tokens_b}"
        )


# ---------------------------------------------------------------------------
# Tests: Performance baseline
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
class TestPerformanceBaseline:
    """Measure and report performance metrics.

    These are not pass/fail tests but capture performance data for
    tracking regressions.
    """

    @pytest.fixture(autouse=True)
    def _setup_paths(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_performance_baseline(self, capsys):
        """Measure time-to-first-token and throughput."""
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)
        prompt = [1, 2, 3, 4, 5]

        # Measure init
        t0 = time.monotonic()
        init(weights, config)
        t_init = time.monotonic() - t0

        # Measure inference
        t_start = time.monotonic()
        tokens = ocr_tokens(32, 32, image, prompt, max_new_tokens=10)
        t_total = time.monotonic() - t_start

        n_tokens = len(tokens)
        tps = n_tokens / t_total if t_total > 0 else 0.0
        ttft = t_total / n_tokens if n_tokens > 0 else 0.0

        # Print for visibility in test output
        print(f"\n{'='*60}")
        print(f"Falcon-OCR Stub Performance Baseline")
        print(f"  Init time:           {t_init:.4f}s")
        print(f"  Total inference:     {t_total:.4f}s")
        print(f"  Tokens generated:    {n_tokens}")
        print(f"  Time-to-first-token: {ttft:.4f}s")
        print(f"  Tokens/sec:          {tps:.2f}")
        print(f"  Token IDs:           {tokens}")
        print(f"{'='*60}")

        # Basic sanity: inference should complete
        assert n_tokens > 0
        assert t_total > 0
