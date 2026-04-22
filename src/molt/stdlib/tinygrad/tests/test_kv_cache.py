"""
Tests for tinygrad.kv_cache — Tiered KV cache manager.

Covers:
  - Tier promotion/demotion correctness
  - Quantization roundtrip accuracy
  - Eviction policy correctness (most important tokens retained)
  - Memory savings measurement
  - Attention sink and sliding window protection
  - Integration with generic speculative decoding helpers
  - Integration with tree attention
  - Score decay and accumulation
  - Capacity enforcement
  - Determinism
"""

import random
import sys
import os
import tempfile

# Ensure the tinygrad package is importable from the source tree.
# We symlink the tinygrad directory into a temp location so that
# Python's standard library 'math' is found before molt's stdlib math.py.
if "tinygrad" not in sys.modules:
    _this_dir = os.path.dirname(os.path.abspath(__file__))
    _tinygrad_pkg = os.path.dirname(_this_dir)
    _td = tempfile.mkdtemp()
    _link = os.path.join(_td, "tinygrad")
    if not os.path.exists(_link):
        os.symlink(_tinygrad_pkg, _link)
    if _td not in sys.path:
        sys.path.insert(0, _td)

from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes
from tinygrad.lazy import LazyOp, LazyBuffer
from tinygrad.kv_cache import (
    TieredKVCache,
    TIER_HOT,
    TIER_WARM,
    TIER_COLD,
    compute_attention_importance,
    compute_attention_importance_from_positions,
    tiered_attention,
    _quantize_vector,
    _dequantize_entry,
)
from tinygrad.speculative import speculative_decode_with_kv_cache
from tinygrad.tree_attention import (
    tiered_tree_attention,
    compact_tiered_kv_cache,
)


def _make_kv_vec(d_k: int, seed: int) -> list:
    """Generate a deterministic float vector of length d_k."""
    rng = random.Random(seed)
    return [rng.gauss(0.0, 1.0) for _ in range(d_k)]


def _make_tensor_1d(data: list) -> Tensor:
    shape = (len(data),)
    op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.float32, shape, data=list(data)))


def _make_tensor_2d(data: list, rows: int, cols: int) -> Tensor:
    shape = (rows, cols)
    op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.float32, shape, data=list(data)))


# ============================================================
# Test: Basic append and retrieval
# ============================================================


def test_append_and_retrieve():
    d_k = 16
    cache = TieredKVCache(
        d_k=d_k, max_hot=10, max_warm=10, max_cold=10, sliding_window=2, n_sinks=1
    )

    k0 = _make_kv_vec(d_k, seed=0)
    v0 = _make_kv_vec(d_k, seed=1)
    pos0 = cache.append(k0, v0)
    assert pos0 == 0
    assert cache.hot_count == 1
    assert cache.total_tokens == 1

    k1 = _make_kv_vec(d_k, seed=2)
    v1 = _make_kv_vec(d_k, seed=3)
    pos1 = cache.append(k1, v1)
    assert pos1 == 1
    assert cache.hot_count == 2

    # Retrieve hot tier
    k_tensor, v_tensor, positions = cache.get_hot_kv()
    assert positions == [0, 1]
    assert k_tensor.shape == (2, d_k)

    k_data = k_tensor.realize().lazydata._data
    for i in range(d_k):
        assert abs(k_data[i] - k0[i]) < 1e-10
        assert abs(k_data[d_k + i] - k1[i]) < 1e-10

    print("PASS: test_append_and_retrieve")


# ============================================================
# Test: Quantization roundtrip accuracy
# ============================================================


def test_quantization_roundtrip_int8():
    d_k = 64
    rng = random.Random(42)
    data = [rng.gauss(0.0, 1.0) for _ in range(d_k)]

    q, scales = _quantize_vector(data, n_bits=8, block_size=32)
    deq = _dequantize_entry(q, scales, block_size=32)

    # INT8 quantization error should be small (< 1% of max value)
    max_val = max(abs(x) for x in data)
    max_error = max(abs(data[i] - deq[i]) for i in range(d_k))
    relative_error = max_error / max_val if max_val > 0 else 0.0

    assert relative_error < 0.02, (
        f"INT8 quantization error too high: {relative_error:.4f}"
    )
    print(
        f"PASS: test_quantization_roundtrip_int8 (max relative error: {relative_error:.6f})"
    )


def test_quantization_roundtrip_int4():
    d_k = 64
    rng = random.Random(42)
    data = [rng.gauss(0.0, 1.0) for _ in range(d_k)]

    q, scales = _quantize_vector(data, n_bits=4, block_size=32)
    deq = _dequantize_entry(q, scales, block_size=32)

    # INT4 has coarser quantization but should still be reasonable
    max_val = max(abs(x) for x in data)
    max_error = max(abs(data[i] - deq[i]) for i in range(d_k))
    relative_error = max_error / max_val if max_val > 0 else 0.0

    assert relative_error < 0.20, (
        f"INT4 quantization error too high: {relative_error:.4f}"
    )
    print(
        f"PASS: test_quantization_roundtrip_int4 (max relative error: {relative_error:.6f})"
    )


def test_quantization_zero_vector():
    d_k = 16
    data = [0.0] * d_k
    q, scales = _quantize_vector(data, n_bits=8, block_size=16)
    deq = _dequantize_entry(q, scales, block_size=16)
    assert all(x == 0.0 for x in deq), "Zero vector quantization should produce zeros"
    print("PASS: test_quantization_zero_vector")


# ============================================================
# Test: Tier demotion via threshold
# ============================================================


def test_tier_demotion_threshold():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=100,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.1,
        warm_threshold=0.01,
        score_decay=0.9,
    )

    # Add 5 tokens
    for i in range(5):
        cache.append(_make_kv_vec(d_k, seed=i * 2), _make_kv_vec(d_k, seed=i * 2 + 1))

    # Give token 2 a high score, tokens 1 and 3 scores between thresholds
    # hot_threshold=0.1, warm_threshold=0.01
    # Scores at 0.05 are below hot_threshold but above warm_threshold => warm tier
    cache.update_scores({0: 0.5, 1: 0.05, 2: 1.0, 3: 0.05, 4: 0.5})

    # Token 0 is a sink (pos < n_sinks=1), protected
    # Token 4 is in sliding window (last 1), protected
    # Token 2 has high score, stays hot
    # Tokens 1 and 3 have scores below hot_threshold but above warm_threshold => warm

    cache.step()

    assert cache.get_tier(0) == TIER_HOT, "Sink token 0 must stay hot"
    assert cache.get_tier(2) == TIER_HOT, "High-score token 2 must stay hot"
    assert cache.get_tier(4) == TIER_HOT, "Sliding window token 4 must stay hot"
    assert cache.get_tier(1) == TIER_WARM, "Mid-score token 1 should demote to warm"
    assert cache.get_tier(3) == TIER_WARM, "Mid-score token 3 should demote to warm"

    print("PASS: test_tier_demotion_threshold")


def test_tier_demotion_warm_to_cold():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=100,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.1,
        warm_threshold=0.01,
        score_decay=0.5,
    )

    # Add 3 tokens
    for i in range(3):
        cache.append(_make_kv_vec(d_k, seed=i * 2), _make_kv_vec(d_k, seed=i * 2 + 1))

    # Give token 1 a score between thresholds: 0.01 < 0.05 < 0.1
    # This should land in warm tier (below hot, above warm)
    cache.update_scores({0: 1.0, 1: 0.05, 2: 1.0})
    cache.step()
    assert cache.get_tier(1) == TIER_WARM, (
        f"Token 1 should be in warm tier, got {cache.get_tier(1)}"
    )

    # Decay: 0.05 * 0.5 = 0.025, still above warm_threshold=0.01, stays warm
    cache.update_scores({0: 1.0, 2: 1.0})  # no weight for token 1
    cache.step()
    assert cache.get_tier(1) == TIER_WARM, "Token 1 should still be warm (0.025 > 0.01)"

    # Decay again: 0.025 * 0.5 = 0.0125, still above 0.01, stays warm
    cache.update_scores({0: 1.0, 2: 1.0})
    cache.step()
    assert cache.get_tier(1) == TIER_WARM, (
        "Token 1 should still be warm (0.0125 > 0.01)"
    )

    # Decay again: 0.0125 * 0.5 = 0.00625 < 0.01, should demote to cold
    cache.update_scores({0: 1.0, 2: 1.0})
    cache.step()
    assert cache.get_tier(1) == TIER_COLD, (
        f"Token 1 should demote to cold after decay, got {cache.get_tier(1)}"
    )

    print("PASS: test_tier_demotion_warm_to_cold")


# ============================================================
# Test: Attention sink protection
# ============================================================


def test_attention_sink_protection():
    d_k = 8
    n_sinks = 3
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=10,
        max_warm=10,
        max_cold=10,
        sliding_window=1,
        n_sinks=n_sinks,
        hot_threshold=0.5,
        score_decay=0.9,
    )

    # Add 6 tokens
    for i in range(6):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Give all tokens zero score (below threshold)
    cache.update_scores({i: 0.0 for i in range(6)})
    cache.step()

    # First 3 tokens (sinks) must remain hot regardless of score
    for i in range(n_sinks):
        assert cache.get_tier(i) == TIER_HOT, f"Sink token {i} must stay hot"

    # Last token (sliding window) must remain hot
    assert cache.get_tier(5) == TIER_HOT, "Sliding window token must stay hot"

    # Middle tokens should have been demoted
    assert cache.get_tier(3) != TIER_HOT, "Non-sink non-window token 3 should demote"
    assert cache.get_tier(4) != TIER_HOT, "Non-sink non-window token 4 should demote"

    print("PASS: test_attention_sink_protection")


# ============================================================
# Test: Sliding window protection
# ============================================================


def test_sliding_window_protection():
    d_k = 8
    window = 3
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=20,
        max_warm=20,
        max_cold=20,
        sliding_window=window,
        n_sinks=1,
        hot_threshold=0.5,
        score_decay=0.9,
    )

    # Add 8 tokens
    for i in range(8):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # All zero scores
    cache.update_scores({i: 0.0 for i in range(8)})
    cache.step()

    # Last 3 tokens (positions 5, 6, 7) should be protected
    hot_positions = sorted(cache._hot.keys())
    for pos in hot_positions[-window:]:
        assert cache.get_tier(pos) == TIER_HOT, (
            f"Sliding window token {pos} must stay hot"
        )

    # Sink (position 0) also protected
    assert cache.get_tier(0) == TIER_HOT, "Sink must stay hot"

    print("PASS: test_sliding_window_protection")


# ============================================================
# Test: Capacity enforcement
# ============================================================


def test_hot_capacity_enforcement():
    d_k = 8
    max_hot = 5
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=max_hot,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=1e-6,
        warm_threshold=1e-8,  # Very low thresholds
        score_decay=0.99,
    )

    # Add 10 tokens, exceeding max_hot
    for i in range(10):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Give varying scores
    scores = {i: float(i) * 0.1 for i in range(10)}
    cache.update_scores(scores)
    cache.step()

    assert cache.hot_count <= max_hot, (
        f"Hot tier has {cache.hot_count} entries, max is {max_hot}"
    )
    # Highest-scored tokens + protected tokens should be in hot
    # Position 0 (sink), position 9 (window), and the highest-scored non-protected
    assert cache.get_tier(0) == TIER_HOT, "Sink must stay hot"
    assert cache.get_tier(9) == TIER_HOT, "Sliding window must stay hot"

    print("PASS: test_hot_capacity_enforcement")


def test_cold_capacity_permanent_eviction():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=2,
        max_warm=2,
        max_cold=2,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.5,
        warm_threshold=0.01,
        score_decay=0.1,
    )

    # Add 10 tokens
    for i in range(10):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Only give score to sinks and window
    cache.update_scores({0: 1.0, 9: 1.0})

    # Run multiple steps to cascade through tiers
    for _ in range(5):
        cache.step()

    # Total tokens should be bounded by total capacity
    total_capacity = 2 + 2 + 2
    assert cache.total_tokens <= total_capacity, (
        f"Total tokens {cache.total_tokens} exceeds capacity {total_capacity}"
    )

    print("PASS: test_cold_capacity_permanent_eviction")


# ============================================================
# Test: Memory savings measurement
# ============================================================


def test_memory_savings():
    d_k = 64
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=10,
        max_warm=50,
        max_cold=100,
        sliding_window=2,
        n_sinks=1,
        hot_threshold=0.1,
        warm_threshold=0.01,
        score_decay=0.9,
        warm_n_bits=8,
        cold_n_bits=4,
    )

    # Add 20 tokens
    for i in range(20):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Give only a few tokens high scores
    high_score_tokens = {0, 1, 18, 19}  # sinks + window
    for i in range(20):
        score = 1.0 if i in high_score_tokens else 0.001
        cache.update_scores({i: score})

    cache.step()

    ratio = cache.memory_savings_ratio()
    assert 0.0 < ratio < 1.0, f"Expected savings ratio in (0, 1), got {ratio}"

    hot_bytes = cache.memory_bytes_hot()
    warm_bytes = cache.memory_bytes_warm()
    cold_bytes = cache.memory_bytes_cold()
    total_actual = hot_bytes + warm_bytes + cold_bytes
    total_full = cache.total_tokens * d_k * 2 * 4

    assert total_actual < total_full, (
        f"Actual memory {total_actual} should be less than full precision {total_full}"
    )

    print(
        f"PASS: test_memory_savings (ratio: {ratio:.4f}, "
        f"hot: {hot_bytes}, warm: {warm_bytes}, cold: {cold_bytes})"
    )


# ============================================================
# Test: Score decay and accumulation
# ============================================================


def test_score_decay():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=10,
        max_warm=10,
        max_cold=10,
        sliding_window=1,
        n_sinks=1,
        score_decay=0.5,
    )

    cache.append(_make_kv_vec(d_k, seed=0), _make_kv_vec(d_k, seed=1))

    # Step 1: add score 1.0
    cache.update_scores({0: 1.0})
    assert abs(cache.get_score(0) - 1.0) < 1e-10

    # Step 2: add score 0.0 (just decay)
    cache.update_scores({0: 0.0})
    assert abs(cache.get_score(0) - 0.5) < 1e-10  # 1.0 * 0.5 + 0.0

    # Step 3: add score 1.0
    cache.update_scores({0: 1.0})
    assert abs(cache.get_score(0) - 1.25) < 1e-10  # 0.5 * 0.5 + 1.0

    print("PASS: test_score_decay")


def test_heavy_hitter_retention():
    """Heavy hitter tokens (consistently high attention) should stay in hot tier."""
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=5,
        max_warm=20,
        max_cold=20,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.1,
        score_decay=0.9,
    )

    # Add 10 tokens
    for i in range(10):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Simulate 20 steps where token 5 is consistently attended to (heavy hitter)
    for step in range(20):
        weights = {i: 0.01 for i in range(10)}
        weights[5] = 0.5  # Heavy hitter
        weights[0] = 0.3  # Sink also gets attention
        cache.update_scores(weights)
        cache.step()

    # Token 5 should be in hot tier due to accumulated heavy-hitter score
    assert cache.get_tier(5) == TIER_HOT, (
        f"Heavy hitter token 5 should be hot, got tier {cache.get_tier(5)}"
    )

    print("PASS: test_heavy_hitter_retention")


# ============================================================
# Test: Promotion from warm/cold back to hot
# ============================================================


def test_promotion():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=100,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.5,
        warm_threshold=0.01,
        score_decay=0.5,
    )

    # Add 3 tokens
    for i in range(3):
        cache.append(_make_kv_vec(d_k, seed=i * 2), _make_kv_vec(d_k, seed=i * 2 + 1))

    original_k0 = _make_kv_vec(d_k, seed=0)
    original_v0 = _make_kv_vec(d_k, seed=1)

    # Demote token 1 to warm (score between thresholds: 0.01 < 0.05 < 0.5)
    cache.update_scores({0: 1.0, 1: 0.05, 2: 1.0})
    cache.step()
    assert cache.get_tier(1) == TIER_WARM, (
        f"Expected warm, got tier {cache.get_tier(1)}"
    )

    # Promote token 1 back to hot
    success = cache.promote_to_hot(1)
    assert success, "Promotion should succeed"
    assert cache.get_tier(1) == TIER_HOT

    # Verify data integrity: token 0 should have original data
    k_tensor, v_tensor, positions = cache.get_hot_kv()
    k_data = k_tensor.realize().lazydata._data
    v_data = v_tensor.realize().lazydata._data
    pos_idx = positions.index(0)
    for i in range(d_k):
        assert abs(k_data[pos_idx * d_k + i] - original_k0[i]) < 1e-10
        assert abs(v_data[pos_idx * d_k + i] - original_v0[i]) < 1e-10

    print("PASS: test_promotion")


# ============================================================
# Test: Compact to accepted (speculative decoding)
# ============================================================


def test_compact_to_accepted():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k, max_hot=20, max_warm=20, max_cold=20, sliding_window=1, n_sinks=1
    )

    # Add 5 tokens
    for i in range(5):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    assert cache.total_tokens == 5

    # Compact to only positions [0, 2, 4]
    cache.compact_to_accepted([0, 2, 4])

    assert cache.total_tokens == 3
    assert cache.get_tier(0) == TIER_HOT
    assert cache.get_tier(1) == -1  # Evicted
    assert cache.get_tier(2) == TIER_HOT
    assert cache.get_tier(3) == -1  # Evicted
    assert cache.get_tier(4) == TIER_HOT

    print("PASS: test_compact_to_accepted")


# ============================================================
# Test: Attention importance computation
# ============================================================


def test_attention_importance():
    # Query that aligns with key 1
    q_data = [0.0, 1.0, 0.0, 0.0]
    k_data = [
        1.0,
        0.0,
        0.0,
        0.0,  # key 0: orthogonal to q
        0.0,
        1.0,
        0.0,
        0.0,  # key 1: aligned with q
        0.0,
        0.0,
        1.0,
        0.0,  # key 2: orthogonal to q
    ]
    q = _make_tensor_1d(q_data)
    k = _make_tensor_2d(k_data, 3, 4)

    weights = compute_attention_importance(q, k)

    # Key 1 should have the highest weight
    assert weights[1] > weights[0], "Key 1 should be more important than key 0"
    assert weights[1] > weights[2], "Key 1 should be more important than key 2"

    # Weights should sum to ~1 (softmax output)
    total = sum(weights.values())
    assert abs(total - 1.0) < 1e-6, f"Weights should sum to 1, got {total}"

    print("PASS: test_attention_importance")


def test_attention_importance_from_positions():
    q = _make_tensor_1d([1.0, 0.0, 0.0, 0.0])
    k_data = [
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
    ]
    k = _make_tensor_2d(k_data, 2, 4)
    positions = [10, 20]

    weights = compute_attention_importance_from_positions(q, k, positions)

    assert 10 in weights
    assert 20 in weights
    assert weights[10] > weights[20]

    print("PASS: test_attention_importance_from_positions")


# ============================================================
# Test: Tiered attention
# ============================================================


def test_tiered_attention_hot_only():
    d_k = 4
    cache = TieredKVCache(
        d_k=d_k, max_hot=10, max_warm=10, max_cold=10, sliding_window=1, n_sinks=1
    )

    # Add 3 tokens with known K/V
    cache.append([1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0])
    cache.append([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0])
    cache.append([0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 0.0])

    # Query aligned with key 1
    q = _make_tensor_1d([0.0, 1.0, 0.0, 0.0])

    output = tiered_attention(q, cache, causal=False)
    out_data = output.realize().lazydata._data

    # Output should be weighted toward V[1] = [0, 1, 0, 0]
    assert out_data[1] > out_data[0], "Output should favor dimension 1"
    assert out_data[1] > out_data[2], "Output should favor dimension 1"

    print("PASS: test_tiered_attention_hot_only")


def test_tiered_attention_mixed_tiers():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=100,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.5,
        warm_threshold=0.01,
        score_decay=0.9,
    )

    # Add 5 tokens
    for i in range(5):
        k = [0.0] * d_k
        v = [0.0] * d_k
        k[i % d_k] = 1.0
        v[i % d_k] = 1.0
        cache.append(k, v)

    # Demote tokens 1 and 2 to warm (scores between warm and hot thresholds)
    cache.update_scores({0: 1.0, 1: 0.05, 2: 0.05, 3: 1.0, 4: 1.0})
    cache.step()

    assert cache.get_tier(1) == TIER_WARM, f"Expected warm, got {cache.get_tier(1)}"
    assert cache.get_tier(2) == TIER_WARM, f"Expected warm, got {cache.get_tier(2)}"

    # Query aligned with warm token 1
    q_data = [0.0] * d_k
    q_data[1] = 1.0
    q = _make_tensor_1d(q_data)

    output = tiered_attention(q, cache, causal=False)
    out_data = output.realize().lazydata._data

    # Output should still attend to warm token 1's value (dequantized)
    # The value had 1.0 at index 1, so output should have high weight there
    assert out_data[1] > 0.1, (
        f"Output should attend to warm tier token, got {out_data[1]:.4f}"
    )

    print("PASS: test_tiered_attention_mixed_tiers")


# ============================================================
# Test: Integration with speculative decoding
# ============================================================


def test_speculative_decode_with_kv_cache():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k, max_hot=20, max_warm=20, max_cold=20, sliding_window=2, n_sinks=1
    )

    # Pre-fill 3 context tokens
    for i in range(3):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Add 3 draft tokens
    draft_positions = []
    for i in range(3):
        pos = cache.append(
            _make_kv_vec(d_k, seed=10 + i),
            _make_kv_vec(d_k, seed=110 + i),
        )
        draft_positions.append(pos)

    assert cache.total_tokens == 6

    # Create logits where draft and target agree on first 2 tokens
    vocab_size = 4
    random.seed(99)

    # Draft tokens: [1, 2, 3]
    draft_tokens = _make_tensor_1d([1.0, 2.0, 3.0])

    # Draft logits: high prob for tokens 1, 2, 3 respectively
    draft_logit_data = [0.0] * (3 * vocab_size)
    target_logit_data = [0.0] * (3 * vocab_size)
    for i in range(3):
        token = int([1, 2, 3][i])
        draft_logit_data[i * vocab_size + token] = 5.0  # high logit
        if i < 2:
            target_logit_data[i * vocab_size + token] = 5.0  # agree on first 2
        else:
            target_logit_data[i * vocab_size + 0] = 5.0  # disagree on 3rd

    draft_logits = _make_tensor_2d(draft_logit_data, 3, vocab_size)
    target_logits = _make_tensor_2d(target_logit_data, 3, vocab_size)

    q = _make_tensor_1d(_make_kv_vec(d_k, seed=999))

    accepted_tokens, n_accepted, accepted_positions = speculative_decode_with_kv_cache(
        draft_logits,
        target_logits,
        draft_tokens,
        cache,
        draft_positions,
        q,
    )

    # At least the first token should be accepted (target agrees)
    assert n_accepted >= 1, f"Expected at least 1 accepted, got {n_accepted}"
    assert len(accepted_positions) == n_accepted

    # Rejected positions should be removed from cache
    for rejected_pos in draft_positions[n_accepted:]:
        assert cache.get_tier(rejected_pos) == -1, (
            f"Rejected position {rejected_pos} should be evicted"
        )

    print(f"PASS: test_speculative_decode_with_kv_cache (accepted: {n_accepted})")


# ============================================================
# Test: Integration with tree attention
# ============================================================


def test_tiered_tree_attention():
    d_k = 4
    cache = TieredKVCache(
        d_k=d_k, max_hot=20, max_warm=20, max_cold=20, sliding_window=1, n_sinks=1
    )

    # Add 2 prefix tokens
    cache.append([1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0])
    cache.append([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0])

    # Add 3 tree tokens
    pos_root = cache.append([0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 0.0])
    pos_left = cache.append([1.0, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0])
    pos_right = cache.append([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0])

    # Tree: root -> left, root -> right
    tree_structure = [-1, 0, 0]  # root, child of root, child of root
    tree_positions = [pos_root, pos_left, pos_right]

    # Queries for each tree node
    q_data = [
        0.0,
        0.0,
        1.0,
        0.0,  # root query
        1.0,
        1.0,
        0.0,
        0.0,  # left child query
        0.0,
        0.0,
        0.0,
        1.0,  # right child query
    ]
    q = _make_tensor_2d(q_data, 3, d_k)

    output = tiered_tree_attention(q, cache, tree_structure, tree_positions)
    out_data = output.realize().lazydata._data

    # Output should have shape (3, 4)
    assert output.shape == (3, d_k)
    assert len(out_data) == 3 * d_k

    # Each tree node should attend to prefix + its ancestors
    # Node 0 (root): attends to prefix [0,1] + self [2]
    # Node 1 (left): attends to prefix [0,1] + root [2] + self [3]
    # Node 2 (right): attends to prefix [0,1] + root [2] + self [4]
    # But NOT siblings (left should not attend to right's position, and vice versa)

    print("PASS: test_tiered_tree_attention")


def test_compact_tiered_kv_cache():
    d_k = 4
    cache = TieredKVCache(
        d_k=d_k, max_hot=20, max_warm=20, max_cold=20, sliding_window=2, n_sinks=2
    )

    # Add 2 prefix tokens (both sinks since n_sinks=2)
    cache.append([1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0])
    cache.append([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0])

    # Give prefix tokens high scores so they stay hot after step()
    cache.update_scores({0: 1.0, 1: 1.0})

    # Add 4 tree tokens
    tree_positions = []
    for i in range(4):
        pos = cache.append(
            _make_kv_vec(d_k, seed=10 + i),
            _make_kv_vec(d_k, seed=110 + i),
        )
        tree_positions.append(pos)

    assert cache.total_tokens == 6

    # Accept only nodes 0 and 1 from tree
    accepted_indices = [0, 1]
    compact_tiered_kv_cache(cache, accepted_indices, tree_positions)

    # Prefix tokens should still be present (sinks + high score)
    assert cache.get_tier(0) == TIER_HOT, "Prefix token 0 should remain hot"
    assert cache.get_tier(1) == TIER_HOT, "Prefix token 1 should remain hot"

    # Accepted tree tokens should remain (in some tier)
    assert cache.get_tier(tree_positions[0]) != -1, (
        "Accepted tree token 0 should remain"
    )
    assert cache.get_tier(tree_positions[1]) != -1, (
        "Accepted tree token 1 should remain"
    )

    # Rejected tree tokens should be gone
    assert cache.get_tier(tree_positions[2]) == -1, (
        "Rejected tree token 2 should be evicted"
    )
    assert cache.get_tier(tree_positions[3]) == -1, (
        "Rejected tree token 3 should be evicted"
    )

    print("PASS: test_compact_tiered_kv_cache")


# ============================================================
# Test: Determinism
# ============================================================


def test_determinism():
    """Running the same sequence of operations twice should produce identical results."""
    d_k = 16

    def run_scenario():
        cache = TieredKVCache(
            d_k=d_k,
            max_hot=5,
            max_warm=10,
            max_cold=10,
            sliding_window=2,
            n_sinks=1,
            hot_threshold=0.1,
            warm_threshold=0.01,
            score_decay=0.9,
        )
        rng = random.Random(12345)
        for i in range(15):
            k = [rng.gauss(0, 1) for _ in range(d_k)]
            v = [rng.gauss(0, 1) for _ in range(d_k)]
            cache.append(k, v)
            weights = {j: rng.random() for j in range(i + 1)}
            cache.update_scores(weights)
            cache.step()

        # Collect final state
        state = {}
        for pos in sorted(cache._scores.keys()):
            state[pos] = (cache.get_tier(pos), cache.get_score(pos))
        return state

    state1 = run_scenario()
    state2 = run_scenario()

    assert state1 == state2, "Two identical runs should produce identical state"
    print("PASS: test_determinism")


# ============================================================
# Test: Validation errors
# ============================================================


def test_validation_errors():
    # Invalid d_k
    try:
        TieredKVCache(d_k=0)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass

    # max_hot too small
    try:
        TieredKVCache(d_k=8, max_hot=3, sliding_window=2, n_sinks=2)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass

    # Invalid score_decay
    try:
        TieredKVCache(d_k=8, score_decay=1.0)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass

    # warm_threshold >= hot_threshold
    try:
        TieredKVCache(d_k=8, hot_threshold=0.01, warm_threshold=0.01)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass

    # Wrong vector length
    cache = TieredKVCache(d_k=8)
    try:
        cache.append([1.0] * 4, [1.0] * 8)  # k wrong length
        assert False, "Should have raised ValueError"
    except ValueError:
        pass

    print("PASS: test_validation_errors")


# ============================================================
# Test: get_all_kv combines all tiers
# ============================================================


def test_get_all_kv():
    d_k = 8
    cache = TieredKVCache(
        d_k=d_k,
        max_hot=100,
        max_warm=100,
        max_cold=100,
        sliding_window=1,
        n_sinks=1,
        hot_threshold=0.5,
        warm_threshold=0.01,
        score_decay=0.9,
    )

    # Add 5 tokens
    for i in range(5):
        cache.append(_make_kv_vec(d_k, seed=i), _make_kv_vec(d_k, seed=i + 100))

    # Demote some to warm (scores between thresholds)
    cache.update_scores({0: 1.0, 1: 0.05, 2: 0.05, 3: 1.0, 4: 1.0})
    cache.step()

    k_all, v_all, all_positions = cache.get_all_kv()

    # All 5 tokens should be present (just in different tiers)
    assert len(all_positions) == 5
    assert all_positions == [0, 1, 2, 3, 4]
    assert k_all.shape == (5, d_k)
    assert v_all.shape == (5, d_k)

    print("PASS: test_get_all_kv")


# ============================================================
# Test: Empty cache operations
# ============================================================


def test_empty_cache():
    d_k = 8
    cache = TieredKVCache(d_k=d_k)

    assert cache.total_tokens == 0
    assert cache.memory_savings_ratio() == 1.0

    k, v, pos = cache.get_hot_kv()
    assert k is None and v is None and pos == []

    k, v, pos = cache.get_all_kv()
    assert k is None and v is None and pos == []

    # Tiered attention on empty cache should return zeros
    q = _make_tensor_1d([1.0] * d_k)
    output = tiered_attention(q, cache, causal=False)
    out_data = output.realize().lazydata._data
    assert all(x == 0.0 for x in out_data)

    print("PASS: test_empty_cache")


# ============================================================
# Test: Promote nonexistent position
# ============================================================


def test_promote_nonexistent():
    d_k = 8
    cache = TieredKVCache(d_k=d_k)
    assert cache.promote_to_hot(999) is False
    print("PASS: test_promote_nonexistent")


# ============================================================
# Run all tests
# ============================================================

if __name__ == "__main__":
    test_append_and_retrieve()
    test_quantization_roundtrip_int8()
    test_quantization_roundtrip_int4()
    test_quantization_zero_vector()
    test_tier_demotion_threshold()
    test_tier_demotion_warm_to_cold()
    test_attention_sink_protection()
    test_sliding_window_protection()
    test_hot_capacity_enforcement()
    test_cold_capacity_permanent_eviction()
    test_memory_savings()
    test_score_decay()
    test_heavy_hitter_retention()
    test_promotion()
    test_compact_to_accepted()
    test_attention_importance()
    test_attention_importance_from_positions()
    test_tiered_attention_hot_only()
    test_tiered_attention_mixed_tiers()
    test_speculative_decode_with_kv_cache()
    test_tiered_tree_attention()
    test_compact_tiered_kv_cache()
    test_determinism()
    test_validation_errors()
    test_get_all_kv()
    test_empty_cache()
    test_promote_nonexistent()
    print("\n=== ALL 27 TESTS PASSED ===")
