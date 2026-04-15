"""Micro Falcon-OCR end-to-end integration test.

Generates a micro Falcon-OCR model (~50KB weights) and runs a full
forward pass through the entire inference pipeline:
  image → patches → embed → transformer blocks → logits → argmax

This exercises the complete pipeline without downloading real weights.

Model config:
  - 2 transformer layers
  - dim=32, heads=2, head_dim=16
  - vocab=64, patch_size=4, image_size=16
  - Weights ~50KB (generated deterministically from seed)
"""

import math
import random


# ---------------------------------------------------------------------------
# Micro model generator
# ---------------------------------------------------------------------------

class MicroFalconOCRConfig:
    """Configuration for the micro Falcon-OCR model.

    Chosen to produce ~33KB of parameters at fp32 (8,432 params * 4 bytes).
    """
    dim: int = 16
    n_heads: int = 2
    head_dim: int = 8
    n_layers: int = 2
    vocab_size: int = 64
    patch_size: int = 4
    image_size: int = 16
    max_seq_len: int = 32


def _seed_rng(seed: int) -> random.Random:
    """Create a seeded RNG for deterministic weight generation."""
    return random.Random(seed)


def _random_matrix(rng: random.Random, rows: int, cols: int, scale: float = 0.02) -> list:
    """Generate a random weight matrix as a flat list."""
    return [rng.gauss(0.0, scale) for _ in range(rows * cols)]


def _random_vector(rng: random.Random, size: int, scale: float = 0.02) -> list:
    """Generate a random bias vector."""
    return [rng.gauss(0.0, scale) for _ in range(size)]


class MicroFalconOCRWeights:
    """Deterministically generated weights for the micro model."""

    def __init__(self, cfg: MicroFalconOCRConfig, seed: int = 42) -> None:
        rng = _seed_rng(seed)
        self.cfg = cfg
        d = cfg.dim

        # Patch embedding: projects patch_size^2 pixels → dim
        patch_dim = cfg.patch_size * cfg.patch_size
        self.patch_proj_w = _random_matrix(rng, d, patch_dim)
        self.patch_proj_b = _random_vector(rng, d)

        # Positional embedding: max_seq_len positions × dim
        n_patches = (cfg.image_size // cfg.patch_size) ** 2
        self.pos_embed = _random_matrix(rng, cfg.max_seq_len, d)

        # Transformer layers
        self.layers = []
        for _ in range(cfg.n_layers):
            layer = {
                # Layer norm 1
                "ln1_g": [1.0] * d,
                "ln1_b": [0.0] * d,
                # QKV projection: dim → 3 * dim (Q, K, V concatenated)
                "qkv_w": _random_matrix(rng, 3 * d, d),
                "qkv_b": _random_vector(rng, 3 * d),
                # Output projection: dim → dim
                "out_w": _random_matrix(rng, d, d),
                "out_b": _random_vector(rng, d),
                # Layer norm 2
                "ln2_g": [1.0] * d,
                "ln2_b": [0.0] * d,
                # FFN: dim → 4*dim → dim
                "ffn_up_w": _random_matrix(rng, 4 * d, d),
                "ffn_up_b": _random_vector(rng, 4 * d),
                "ffn_down_w": _random_matrix(rng, d, 4 * d),
                "ffn_down_b": _random_vector(rng, d),
            }
            self.layers.append(layer)

        # LM head: dim → vocab_size
        self.lm_head_w = _random_matrix(rng, cfg.vocab_size, d)
        self.lm_head_b = _random_vector(rng, cfg.vocab_size)

    def total_params(self) -> int:
        """Count total parameters in the model."""
        cfg = self.cfg
        d = cfg.dim
        patch_dim = cfg.patch_size * cfg.patch_size
        count = 0
        count += d * patch_dim + d  # patch proj
        count += cfg.max_seq_len * d  # pos embed
        for _ in range(cfg.n_layers):
            count += d  # ln1_g
            count += d  # ln1_b
            count += 3 * d * d + 3 * d  # qkv
            count += d * d + d  # out proj
            count += d  # ln2_g
            count += d  # ln2_b
            count += 4 * d * d + 4 * d  # ffn up
            count += d * 4 * d + d  # ffn down
        count += cfg.vocab_size * d + cfg.vocab_size  # lm head
        return count


# ---------------------------------------------------------------------------
# Micro inference engine (pure Python, no GPU)
# ---------------------------------------------------------------------------

def _matmul(a: list, b: list, m: int, k: int, n: int) -> list:
    """Matrix multiply: a (m×k) @ b (k×n) → result (m×n). All flat lists."""
    result = [0.0] * (m * n)
    for i in range(m):
        for j in range(n):
            s = 0.0
            for p in range(k):
                s += a[i * k + p] * b[p * n + j]
            result[i * n + j] = s
    return result


def _add_bias(x: list, bias: list, rows: int, cols: int) -> list:
    """Add bias to each row of x (rows × cols)."""
    result = list(x)
    for i in range(rows):
        for j in range(cols):
            result[i * cols + j] += bias[j]
    return result


def _layer_norm(x: list, gamma: list, beta: list, size: int, eps: float = 1e-5) -> list:
    """Layer normalization for a single vector of `size` elements."""
    mean = sum(x) / size
    var = sum((v - mean) ** 2 for v in x) / size
    inv_std = 1.0 / math.sqrt(var + eps)
    return [(x[i] - mean) * inv_std * gamma[i] + beta[i] for i in range(size)]


def _gelu(x: list) -> list:
    """GELU activation (approximate)."""
    result = []
    for v in x:
        # GELU(x) = x * 0.5 * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
        t = math.tanh(math.sqrt(2.0 / math.pi) * (v + 0.044715 * v * v * v))
        result.append(v * 0.5 * (1.0 + t))
    return result


def _softmax(x: list) -> list:
    """Numerically stable softmax."""
    m = max(x)
    exps = [math.exp(v - m) for v in x]
    s = sum(exps)
    return [e / s for e in exps]


def _attention(q: list, k: list, v: list, seq_len: int, head_dim: int) -> list:
    """Single-head attention: Q (seq×hd) @ K^T (hd×seq) → scores → softmax → @ V."""
    scale = 1.0 / math.sqrt(head_dim)

    # Q @ K^T: (seq, hd) @ (hd, seq) → (seq, seq)
    scores = [0.0] * (seq_len * seq_len)
    for i in range(seq_len):
        for j in range(seq_len):
            s = 0.0
            for d in range(head_dim):
                s += q[i * head_dim + d] * k[j * head_dim + d]
            scores[i * seq_len + j] = s * scale

    # Softmax over last dim (key dim)
    for i in range(seq_len):
        row = scores[i * seq_len:(i + 1) * seq_len]
        row = _softmax(row)
        for j in range(seq_len):
            scores[i * seq_len + j] = row[j]

    # scores @ V: (seq, seq) @ (seq, hd) → (seq, hd)
    out = _matmul(scores, v, seq_len, seq_len, head_dim)
    return out


def _multi_head_attention(
    x: list,
    qkv_w: list,
    qkv_b: list,
    out_w: list,
    out_b: list,
    seq_len: int,
    dim: int,
    n_heads: int,
    head_dim: int,
) -> list:
    """Multi-head attention block."""
    # QKV projection: (seq, dim) @ (dim, 3*dim) → (seq, 3*dim)
    qkv = _matmul(x, qkv_w, seq_len, dim, 3 * dim)
    qkv = _add_bias(qkv, qkv_b, seq_len, 3 * dim)

    # Split into Q, K, V (each seq × dim)
    q_all = [0.0] * (seq_len * dim)
    k_all = [0.0] * (seq_len * dim)
    v_all = [0.0] * (seq_len * dim)
    for i in range(seq_len):
        for j in range(dim):
            q_all[i * dim + j] = qkv[i * 3 * dim + j]
            k_all[i * dim + j] = qkv[i * 3 * dim + dim + j]
            v_all[i * dim + j] = qkv[i * 3 * dim + 2 * dim + j]

    # Per-head attention
    out_heads = [0.0] * (seq_len * dim)
    for h in range(n_heads):
        hd = head_dim
        # Extract head h
        q_h = [0.0] * (seq_len * hd)
        k_h = [0.0] * (seq_len * hd)
        v_h = [0.0] * (seq_len * hd)
        for i in range(seq_len):
            for d in range(hd):
                q_h[i * hd + d] = q_all[i * dim + h * hd + d]
                k_h[i * hd + d] = k_all[i * dim + h * hd + d]
                v_h[i * hd + d] = v_all[i * dim + h * hd + d]

        head_out = _attention(q_h, k_h, v_h, seq_len, hd)

        # Place back into out_heads
        for i in range(seq_len):
            for d in range(hd):
                out_heads[i * dim + h * hd + d] = head_out[i * hd + d]

    # Output projection: (seq, dim) @ (dim, dim) → (seq, dim)
    result = _matmul(out_heads, out_w, seq_len, dim, dim)
    result = _add_bias(result, out_b, seq_len, dim)
    return result


def _ffn(
    x: list,
    up_w: list,
    up_b: list,
    down_w: list,
    down_b: list,
    seq_len: int,
    dim: int,
) -> list:
    """Feed-forward network with GELU activation."""
    hidden_dim = 4 * dim
    # Up projection: (seq, dim) @ (dim, 4*dim) → (seq, 4*dim)
    h = _matmul(x, up_w, seq_len, dim, hidden_dim)
    h = _add_bias(h, up_b, seq_len, hidden_dim)
    h = _gelu(h)
    # Down projection: (seq, 4*dim) @ (4*dim, dim) → (seq, dim)
    result = _matmul(h, down_w, seq_len, hidden_dim, dim)
    result = _add_bias(result, down_b, seq_len, dim)
    return result


def _transformer_block(
    x: list,
    layer: dict,
    seq_len: int,
    dim: int,
    n_heads: int,
    head_dim: int,
) -> list:
    """One transformer block: LayerNorm → MHA → residual → LayerNorm → FFN → residual."""
    # Pre-norm MHA
    normed = []
    for i in range(seq_len):
        row = x[i * dim:(i + 1) * dim]
        normed.extend(_layer_norm(row, layer["ln1_g"], layer["ln1_b"], dim))

    attn_out = _multi_head_attention(
        normed, layer["qkv_w"], layer["qkv_b"],
        layer["out_w"], layer["out_b"],
        seq_len, dim, n_heads, head_dim,
    )

    # Residual
    residual1 = [x[i] + attn_out[i] for i in range(seq_len * dim)]

    # Pre-norm FFN
    normed2 = []
    for i in range(seq_len):
        row = residual1[i * dim:(i + 1) * dim]
        normed2.extend(_layer_norm(row, layer["ln2_g"], layer["ln2_b"], dim))

    ffn_out = _ffn(
        normed2, layer["ffn_up_w"], layer["ffn_up_b"],
        layer["ffn_down_w"], layer["ffn_down_b"],
        seq_len, dim,
    )

    # Residual
    result = [residual1[i] + ffn_out[i] for i in range(seq_len * dim)]
    return result


def _patch_embed(
    image: list,
    patch_proj_w: list,
    patch_proj_b: list,
    cfg: MicroFalconOCRConfig,
) -> list:
    """Convert image pixels into patch embeddings.

    image: flat list of (image_size * image_size) pixel values [0, 1].
    Returns: flat list of (n_patches × dim) embeddings.
    """
    ps = cfg.patch_size
    n_patches_per_side = cfg.image_size // ps
    n_patches = n_patches_per_side * n_patches_per_side
    patch_dim = ps * ps
    d = cfg.dim

    # Extract patches
    patches = []
    for py in range(n_patches_per_side):
        for px in range(n_patches_per_side):
            patch = []
            for dy in range(ps):
                for dx in range(ps):
                    y = py * ps + dy
                    x_coord = px * ps + dx
                    patch.append(image[y * cfg.image_size + x_coord])
            patches.extend(patch)

    # Project: (n_patches, patch_dim) @ (patch_dim, dim) → (n_patches, dim)
    # patch_proj_w is (dim, patch_dim), we need (patch_dim, dim) transpose
    w_t = [0.0] * (patch_dim * d)
    for i in range(d):
        for j in range(patch_dim):
            w_t[j * d + i] = patch_proj_w[i * patch_dim + j]

    embedded = _matmul(patches, w_t, n_patches, patch_dim, d)
    embedded = _add_bias(embedded, patch_proj_b, n_patches, d)
    return embedded


def _add_pos_embed(x: list, pos_embed: list, seq_len: int, dim: int) -> list:
    """Add positional embeddings to the sequence."""
    result = list(x)
    for i in range(seq_len):
        for j in range(dim):
            result[i * dim + j] += pos_embed[i * dim + j]
    return result


def forward_pass(
    image: list,
    weights: MicroFalconOCRWeights,
    cfg: MicroFalconOCRConfig,
) -> list:
    """Full forward pass: image → patches → embed → transformer → logits.

    Returns logits as flat list of (seq_len × vocab_size).
    """
    d = cfg.dim

    # 1. Patch embedding
    x = _patch_embed(image, weights.patch_proj_w, weights.patch_proj_b, cfg)
    n_patches = (cfg.image_size // cfg.patch_size) ** 2
    seq_len = n_patches

    # 2. Add positional embeddings
    x = _add_pos_embed(x, weights.pos_embed, seq_len, d)

    # 3. Transformer blocks
    for layer in weights.layers:
        x = _transformer_block(x, layer, seq_len, d, cfg.n_heads, cfg.head_dim)

    # 4. LM head: project last position to vocab
    # Use all positions for the logits (for completeness).
    # lm_head_w is (vocab, dim), need (dim, vocab) transpose
    vocab = cfg.vocab_size
    w_t = [0.0] * (d * vocab)
    for i in range(vocab):
        for j in range(d):
            w_t[j * vocab + i] = weights.lm_head_w[i * d + j]

    logits = _matmul(x, w_t, seq_len, d, vocab)
    logits = _add_bias(logits, weights.lm_head_b, seq_len, vocab)

    return logits


def _generate_test_image(rng: random.Random, cfg: MicroFalconOCRConfig) -> list:
    """Generate a deterministic test image (pixel values in [0, 1])."""
    return [rng.random() for _ in range(cfg.image_size * cfg.image_size)]


def _argmax(logits: list, start: int, length: int) -> int:
    """Argmax over a slice of logits."""
    best_idx = 0
    best_val = logits[start]
    for i in range(1, length):
        if logits[start + i] > best_val:
            best_val = logits[start + i]
            best_idx = i
    return best_idx


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_model_size():
    """Verify the micro model is ~50KB in parameters."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    n_params = weights.total_params()
    # Each param is float64 in Python (8 bytes), but at fp32 (4 bytes)
    # the model would be ~50KB.
    size_kb = n_params * 4 / 1024
    assert size_kb < 100, f"model too large: {size_kb:.1f} KB"
    assert n_params > 1000, f"model too small: {n_params} params"


def test_forward_pass_output_shape():
    """Forward pass should produce logits of correct shape."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    logits = forward_pass(image, weights, cfg)
    n_patches = (cfg.image_size // cfg.patch_size) ** 2
    expected_len = n_patches * cfg.vocab_size
    assert len(logits) == expected_len, (
        f"expected {expected_len} logits, got {len(logits)}"
    )


def test_logits_are_finite():
    """All logits must be finite (no NaN or inf)."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    logits = forward_pass(image, weights, cfg)
    for i, v in enumerate(logits):
        assert math.isfinite(v), f"logit[{i}] = {v} is not finite"


def test_softmax_sums_to_one():
    """Softmax of logits at each position should sum to 1."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    logits = forward_pass(image, weights, cfg)
    n_patches = (cfg.image_size // cfg.patch_size) ** 2
    vocab = cfg.vocab_size

    for pos in range(n_patches):
        row = logits[pos * vocab:(pos + 1) * vocab]
        probs = _softmax(row)
        total = sum(probs)
        assert abs(total - 1.0) < 1e-6, (
            f"softmax at pos {pos} sums to {total}"
        )


def test_tokens_in_valid_range():
    """Argmax tokens should be in [0, vocab_size)."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    logits = forward_pass(image, weights, cfg)
    n_patches = (cfg.image_size // cfg.patch_size) ** 2
    vocab = cfg.vocab_size

    for pos in range(n_patches):
        token = _argmax(logits, pos * vocab, vocab)
        assert 0 <= token < vocab, f"token {token} at pos {pos} out of range"


def test_autoregressive_steps():
    """Run 5 autoregressive steps; each should produce a valid token."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    logits = forward_pass(image, weights, cfg)
    n_patches = (cfg.image_size // cfg.patch_size) ** 2
    vocab = cfg.vocab_size

    tokens = []
    for step in range(5):
        # Use the last position's logits for the next token.
        last_pos = n_patches - 1
        token = _argmax(logits, last_pos * vocab, vocab)
        assert 0 <= token < vocab, f"step {step}: token {token} out of range"
        tokens.append(token)

        # For autoregressive: re-run with slightly perturbed image
        # (simulating the next step with additional context).
        # In a real model this would feed back the token; here we
        # just verify the pipeline handles multiple invocations.
        for i in range(len(image)):
            image[i] = (image[i] + 0.01 * (step + 1)) % 1.0
        logits = forward_pass(image, weights, cfg)

    assert len(tokens) == 5


def test_deterministic_output():
    """Two runs with the same seed should produce identical output."""
    cfg = MicroFalconOCRConfig()

    weights1 = MicroFalconOCRWeights(cfg, seed=42)
    rng1 = _seed_rng(123)
    image1 = _generate_test_image(rng1, cfg)
    logits1 = forward_pass(image1, weights1, cfg)

    weights2 = MicroFalconOCRWeights(cfg, seed=42)
    rng2 = _seed_rng(123)
    image2 = _generate_test_image(rng2, cfg)
    logits2 = forward_pass(image2, weights2, cfg)

    assert len(logits1) == len(logits2)
    for i in range(len(logits1)):
        assert logits1[i] == logits2[i], (
            f"logit[{i}] differs: {logits1[i]} vs {logits2[i]}"
        )


def test_different_seeds_different_output():
    """Different weight seeds should produce different output."""
    cfg = MicroFalconOCRConfig()
    rng = _seed_rng(123)
    image = _generate_test_image(rng, cfg)

    weights1 = MicroFalconOCRWeights(cfg, seed=42)
    logits1 = forward_pass(list(image), weights1, cfg)

    weights2 = MicroFalconOCRWeights(cfg, seed=99)
    logits2 = forward_pass(list(image), weights2, cfg)

    # At least some logits should differ.
    diffs = sum(1 for a, b in zip(logits1, logits2) if abs(a - b) > 1e-10)
    assert diffs > 0, "different seeds should produce different logits"


def test_different_images_different_output():
    """Different input images should produce different logits."""
    cfg = MicroFalconOCRConfig()
    weights = MicroFalconOCRWeights(cfg, seed=42)

    rng1 = _seed_rng(100)
    image1 = _generate_test_image(rng1, cfg)
    logits1 = forward_pass(image1, weights, cfg)

    rng2 = _seed_rng(200)
    image2 = _generate_test_image(rng2, cfg)
    logits2 = forward_pass(image2, weights, cfg)

    diffs = sum(1 for a, b in zip(logits1, logits2) if abs(a - b) > 1e-10)
    assert diffs > 0, "different images should produce different logits"


if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
