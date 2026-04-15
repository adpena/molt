"""Tests for FlashAttention-3 tiled attention with online softmax."""

import math
import random

from tinygrad.tensor import Tensor
from tinygrad.flash_attention import flash_attention_v3, naive_attention


def _allclose(a: list, b: list, atol: float = 1e-3) -> bool:
    """Check if two flat lists are element-wise close."""
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if abs(x - y) > atol:
            return False
    return True


def _flatten(nested: list) -> list:
    """Flatten a nested list to a flat list."""
    result = []
    for item in nested:
        if isinstance(item, list):
            result.extend(_flatten(item))
        else:
            result.append(item)
    return result


def _random_tensor(rows: int, cols: int, seed: int = 42) -> Tensor:
    """Create a random tensor with reproducible values."""
    rng = random.Random(seed)
    data = [[rng.gauss(0, 1) for _ in range(cols)] for _ in range(rows)]
    return Tensor(data)


def test_flash_v3_matches_naive_small():
    """FlashAttention-3 matches naive attention on small matrices."""
    q = _random_tensor(4, 8, seed=1)
    k = _random_tensor(4, 8, seed=2)
    v = _random_tensor(4, 8, seed=3)

    flash_out = flash_attention_v3(q, k, v, causal=False)
    naive_out = naive_attention(q, k, v, causal=False)

    flash_flat = _flatten(flash_out.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(flash_flat, naive_flat, atol=1e-3), (
        f"FlashAttention-3 vs naive mismatch (non-causal, 4x8)\n"
        f"max diff: {max(abs(a - b) for a, b in zip(flash_flat, naive_flat))}"
    )


def test_flash_v3_matches_naive_causal():
    """FlashAttention-3 matches naive attention with causal masking."""
    q = _random_tensor(6, 4, seed=10)
    k = _random_tensor(6, 4, seed=11)
    v = _random_tensor(6, 4, seed=12)

    flash_out = flash_attention_v3(q, k, v, causal=True)
    naive_out = naive_attention(q, k, v, causal=True)

    flash_flat = _flatten(flash_out.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(flash_flat, naive_flat, atol=1e-3), (
        f"FlashAttention-3 vs naive mismatch (causal, 6x4)\n"
        f"max diff: {max(abs(a - b) for a, b in zip(flash_flat, naive_flat))}"
    )


def test_flash_v3_matches_naive_larger():
    """FlashAttention-3 matches naive on larger matrices (16x32)."""
    q = _random_tensor(16, 32, seed=20)
    k = _random_tensor(16, 32, seed=21)
    v = _random_tensor(16, 32, seed=22)

    flash_out = flash_attention_v3(q, k, v, causal=False)
    naive_out = naive_attention(q, k, v, causal=False)

    flash_flat = _flatten(flash_out.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(flash_flat, naive_flat, atol=1e-3), (
        f"FlashAttention-3 vs naive mismatch (non-causal, 16x32)\n"
        f"max diff: {max(abs(a - b) for a, b in zip(flash_flat, naive_flat))}"
    )


def test_flash_v3_custom_block_sizes():
    """FlashAttention-3 works with custom block sizes."""
    q = _random_tensor(8, 16, seed=30)
    k = _random_tensor(8, 16, seed=31)
    v = _random_tensor(8, 16, seed=32)

    # Small block sizes to test tiling
    flash_out_small = flash_attention_v3(q, k, v, block_br=2, block_bc=2)
    flash_out_large = flash_attention_v3(q, k, v, block_br=8, block_bc=8)
    naive_out = naive_attention(q, k, v)

    small_flat = _flatten(flash_out_small.tolist())
    large_flat = _flatten(flash_out_large.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(small_flat, naive_flat, atol=1e-3), (
        "Small block size produces incorrect results"
    )
    assert _allclose(large_flat, naive_flat, atol=1e-3), (
        "Large block size produces incorrect results"
    )
    assert _allclose(small_flat, large_flat, atol=1e-3), (
        "Different block sizes produce different results"
    )


def test_flash_v3_causal_larger():
    """FlashAttention-3 causal on larger matrices (12x16)."""
    q = _random_tensor(12, 16, seed=40)
    k = _random_tensor(12, 16, seed=41)
    v = _random_tensor(12, 16, seed=42)

    flash_out = flash_attention_v3(q, k, v, causal=True, block_br=4, block_bc=4)
    naive_out = naive_attention(q, k, v, causal=True)

    flash_flat = _flatten(flash_out.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(flash_flat, naive_flat, atol=1e-3), (
        f"FlashAttention-3 vs naive mismatch (causal, 12x16)\n"
        f"max diff: {max(abs(a - b) for a, b in zip(flash_flat, naive_flat))}"
    )


def test_flash_v3_output_shape():
    """FlashAttention-3 preserves input shape."""
    q = _random_tensor(7, 11, seed=50)
    k = _random_tensor(7, 11, seed=51)
    v = _random_tensor(7, 11, seed=52)

    out = flash_attention_v3(q, k, v)
    assert out.shape == (7, 11), f"Expected (7, 11), got {out.shape}"


def test_flash_v3_single_row():
    """FlashAttention-3 handles single-row input."""
    q = _random_tensor(1, 8, seed=60)
    k = _random_tensor(1, 8, seed=61)
    v = _random_tensor(1, 8, seed=62)

    flash_out = flash_attention_v3(q, k, v)
    naive_out = naive_attention(q, k, v)

    flash_flat = _flatten(flash_out.tolist())
    naive_flat = _flatten(naive_out.tolist())

    assert _allclose(flash_flat, naive_flat, atol=1e-3), (
        "Single-row attention mismatch"
    )


def test_flash_v3_different_head_dims():
    """FlashAttention-3 works with various head dimensions."""
    for d_k in [4, 8, 16, 32, 64]:
        q = _random_tensor(8, d_k, seed=70 + d_k)
        k = _random_tensor(8, d_k, seed=80 + d_k)
        v = _random_tensor(8, d_k, seed=90 + d_k)

        flash_out = flash_attention_v3(q, k, v)
        naive_out = naive_attention(q, k, v)

        flash_flat = _flatten(flash_out.tolist())
        naive_flat = _flatten(naive_out.tolist())

        assert _allclose(flash_flat, naive_flat, atol=1e-3), (
            f"Mismatch for d_k={d_k}"
        )


def test_flash_v3_rejects_wrong_dims():
    """FlashAttention-3 rejects non-2D tensors."""
    try:
        flash_attention_v3(Tensor([1.0]), Tensor([1.0]), Tensor([1.0]))
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


if __name__ == "__main__":
    test_flash_v3_matches_naive_small()
    test_flash_v3_matches_naive_causal()
    test_flash_v3_matches_naive_larger()
    test_flash_v3_custom_block_sizes()
    test_flash_v3_causal_larger()
    test_flash_v3_output_shape()
    test_flash_v3_single_row()
    test_flash_v3_different_head_dims()
    test_flash_v3_rejects_wrong_dims()
    print("All FlashAttention-3 tests passed.")
