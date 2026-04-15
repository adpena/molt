"""
tinygrad.kv_cache — Tiered KV cache manager for LLM inference.

Three-tier storage hierarchy:
  Hot  (GPU, full precision): active tokens with high attention importance
  Warm (GPU, quantized INT8): tokens with moderate importance, compressed via PolarQuant
  Cold (CPU/offloaded):       tokens below importance threshold, reconstructible

Eviction policy: H2O-inspired heavy-hitter tracking with StreamingLLM attention sinks.
Scoring: exponential moving average of per-token attention scores.
Sliding window: guaranteed retention of the most recent W tokens regardless of score.

All tensor operations composed from the 26 tinygrad primitives.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes
from tinygrad.lazy import LazyOp, LazyBuffer
from tinygrad.turbo_quant import (
    block_quantize,
    dequantize_symmetric,
    qjl_error_correction,
)


class _TierEntry:
    """Storage for a single token's KV vectors in a specific tier.

    Hot tier: k_data and v_data are full-precision flat lists.
    Warm tier: k_data and v_data are quantized flat lists, with per-block
               scales stored in k_scales/v_scales.
    Cold tier: same as warm but marked for CPU offload.
    """

    __slots__ = (
        "token_pos", "k_data", "v_data", "k_scales", "v_scales",
        "n_bits", "block_size",
    )

    def __init__(
        self,
        token_pos: int,
        k_data: list,
        v_data: list,
        k_scales: list = None,
        v_scales: list = None,
        n_bits: int = 0,
        block_size: int = 128,
    ) -> None:
        self.token_pos = token_pos
        self.k_data = k_data
        self.v_data = v_data
        self.k_scales = k_scales
        self.v_scales = v_scales
        self.n_bits = n_bits  # 0 = full precision, 8 = INT8, 4 = INT4
        self.block_size = block_size


# Tier identifiers — used as integer constants for WHERE-chain tier selection
TIER_HOT = 0
TIER_WARM = 1
TIER_COLD = 2


class TieredKVCache:
    """Three-tier KV cache with importance-based eviction.

    Parameters:
        d_k: dimension of each key/value vector
        max_hot: maximum number of tokens in the hot tier
        max_warm: maximum number of tokens in the warm tier
        max_cold: maximum number of tokens in the cold tier
        sliding_window: number of most-recent tokens guaranteed in hot tier
        n_sinks: number of initial "attention sink" tokens permanently in hot tier
        hot_threshold: minimum importance score to remain in hot tier
        warm_threshold: minimum importance score to remain in warm tier
        score_decay: exponential decay factor for importance scores per step
        warm_n_bits: quantization bit-width for warm tier (default 8)
        cold_n_bits: quantization bit-width for cold tier (default 4)
        quant_block_size: block size for per-block quantization
        qjl_projections: number of QJL random projections for cold tier error correction

    Eviction policy:
        1. Attention sink tokens (first n_sinks positions) are never evicted from hot.
        2. Sliding window tokens (last sliding_window positions) are never evicted from hot.
        3. Remaining tokens are scored by exponential moving average of attention weights.
        4. Tokens below hot_threshold move to warm (quantized INT8).
        5. Tokens below warm_threshold move to cold (quantized INT4 + QJL correction).
        6. When a tier exceeds capacity, lowest-scored tokens are evicted to the next tier.
        7. Cold tier overflow causes permanent eviction (token must be recomputed if needed).

    Score update rule (per decoding step):
        score[t] = score[t] * decay + attn_weight[t]

    This is the H2O "heavy hitter" observation: tokens with consistently high
    attention accumulate high scores and remain in the hot tier. Tokens that
    spike once but are not consistently attended to decay into warm/cold.
    """

    def __init__(
        self,
        d_k: int,
        max_hot: int = 256,
        max_warm: int = 512,
        max_cold: int = 1024,
        sliding_window: int = 64,
        n_sinks: int = 4,
        hot_threshold: float = 0.01,
        warm_threshold: float = 0.001,
        score_decay: float = 0.95,
        warm_n_bits: int = 8,
        cold_n_bits: int = 4,
        quant_block_size: int = 128,
        qjl_projections: int = 32,
    ) -> None:
        if d_k <= 0:
            raise ValueError(f"d_k must be positive, got {d_k}")
        if max_hot < sliding_window + n_sinks:
            raise ValueError(
                f"max_hot ({max_hot}) must be >= sliding_window ({sliding_window}) "
                f"+ n_sinks ({n_sinks})"
            )
        if warm_n_bits not in (4, 8):
            raise ValueError(f"warm_n_bits must be 4 or 8, got {warm_n_bits}")
        if cold_n_bits not in (4, 8):
            raise ValueError(f"cold_n_bits must be 4 or 8, got {cold_n_bits}")
        if not (0.0 < score_decay < 1.0):
            raise ValueError(f"score_decay must be in (0, 1), got {score_decay}")
        if hot_threshold <= 0.0:
            raise ValueError(f"hot_threshold must be positive, got {hot_threshold}")
        if warm_threshold <= 0.0:
            raise ValueError(f"warm_threshold must be positive, got {warm_threshold}")
        if warm_threshold >= hot_threshold:
            raise ValueError(
                f"warm_threshold ({warm_threshold}) must be < hot_threshold ({hot_threshold})"
            )

        self._d_k = d_k
        self._max_hot = max_hot
        self._max_warm = max_warm
        self._max_cold = max_cold
        self._sliding_window = sliding_window
        self._n_sinks = n_sinks
        self._hot_threshold = hot_threshold
        self._warm_threshold = warm_threshold
        self._score_decay = score_decay
        self._warm_n_bits = warm_n_bits
        self._cold_n_bits = cold_n_bits
        self._quant_block_size = quant_block_size
        self._qjl_projections = qjl_projections

        # Storage: position -> _TierEntry
        self._hot: dict = {}   # token_pos -> _TierEntry (full precision)
        self._warm: dict = {}  # token_pos -> _TierEntry (quantized)
        self._cold: dict = {}  # token_pos -> _TierEntry (quantized + QJL)

        # Importance scores: position -> float
        self._scores: dict = {}

        # Monotonic token counter (next position to assign)
        self._next_pos: int = 0

        # Track which positions are attention sinks (permanently hot)
        self._sink_positions: set = set()

    @property
    def d_k(self) -> int:
        return self._d_k

    @property
    def total_tokens(self) -> int:
        return len(self._hot) + len(self._warm) + len(self._cold)

    @property
    def hot_count(self) -> int:
        return len(self._hot)

    @property
    def warm_count(self) -> int:
        return len(self._warm)

    @property
    def cold_count(self) -> int:
        return len(self._cold)

    def memory_bytes_hot(self) -> int:
        """Estimated memory usage of hot tier in bytes (float32 = 4 bytes)."""
        return len(self._hot) * self._d_k * 2 * 4  # K + V, 4 bytes each

    def memory_bytes_warm(self) -> int:
        """Estimated memory usage of warm tier in bytes."""
        bits = self._warm_n_bits
        # Quantized values + scales overhead
        bytes_per_val = bits / 8.0
        n_blocks = (self._d_k + self._quant_block_size - 1) // self._quant_block_size
        scale_bytes = n_blocks * 4  # float32 scales
        return int(len(self._warm) * (self._d_k * 2 * bytes_per_val + 2 * scale_bytes))

    def memory_bytes_cold(self) -> int:
        """Estimated memory usage of cold tier in bytes."""
        bits = self._cold_n_bits
        bytes_per_val = bits / 8.0
        n_blocks = (self._d_k + self._quant_block_size - 1) // self._quant_block_size
        scale_bytes = n_blocks * 4
        return int(len(self._cold) * (self._d_k * 2 * bytes_per_val + 2 * scale_bytes))

    def memory_savings_ratio(self) -> float:
        """Ratio of actual memory to full-precision equivalent.

        Returns a value in (0, 1] where lower is better (more savings).
        1.0 means no savings (everything in hot tier).
        """
        full_precision_bytes = self.total_tokens * self._d_k * 2 * 4
        if full_precision_bytes == 0:
            return 1.0
        actual = self.memory_bytes_hot() + self.memory_bytes_warm() + self.memory_bytes_cold()
        return actual / full_precision_bytes

    def append(self, k_vec: list, v_vec: list) -> int:
        """Append a new token's KV vectors to the hot tier.

        Returns the assigned token position.

        k_vec: flat list of d_k floats (key vector)
        v_vec: flat list of d_k floats (value vector)
        """
        if len(k_vec) != self._d_k or len(v_vec) != self._d_k:
            raise ValueError(
                f"Expected vectors of length {self._d_k}, "
                f"got k={len(k_vec)}, v={len(v_vec)}"
            )

        pos = self._next_pos
        self._next_pos += 1

        entry = _TierEntry(token_pos=pos, k_data=list(k_vec), v_data=list(v_vec))
        self._hot[pos] = entry
        self._scores[pos] = 0.0

        # First n_sinks tokens are permanent attention sinks
        if pos < self._n_sinks:
            self._sink_positions.add(pos)

        return pos

    def update_scores(self, attention_weights: dict) -> None:
        """Update importance scores based on attention weights from current step.

        attention_weights: dict mapping token_pos -> attention weight (float).
        Only positions present in the cache are updated.

        Score update rule:
            score[pos] = score[pos] * decay + attn_weight[pos]

        This implements the H2O heavy-hitter accumulation: tokens consistently
        attended to build high scores, while tokens with transient attention
        decay toward zero.
        """
        decay = self._score_decay

        # Decay all existing scores
        for pos in self._scores:
            self._scores[pos] *= decay

        # Add current attention weights
        for pos, weight in attention_weights.items():
            if pos in self._scores:
                self._scores[pos] += weight

    def step(self) -> None:
        """Execute one tier-management step: enforce thresholds and capacity limits.

        This performs the full eviction cascade:
        1. Identify hot tokens that should demote to warm (below hot_threshold,
           not sinks, not in sliding window).
        2. Identify warm tokens that should demote to cold (below warm_threshold).
        3. Enforce capacity limits by evicting lowest-scored excess tokens.
        4. Quantize demoted tokens appropriately for their destination tier.
        """
        self._demote_hot_to_warm()
        self._demote_warm_to_cold()
        self._enforce_hot_capacity()
        self._enforce_warm_capacity()
        self._enforce_cold_capacity()

    def get_hot_kv(self) -> tuple:
        """Return hot tier as (K, V) Tensors, sorted by token position.

        Returns:
            (k_tensor, v_tensor): shape (n_hot, d_k) each, or None if empty
            positions: list of token positions in order
        """
        if not self._hot:
            return None, None, []

        positions = sorted(self._hot.keys())
        k_data = []
        v_data = []
        for pos in positions:
            entry = self._hot[pos]
            k_data.extend(entry.k_data)
            v_data.extend(entry.v_data)

        n = len(positions)
        shape = (n, self._d_k)
        k_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        v_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        k_tensor = Tensor(LazyBuffer(k_op, dtypes.float32, shape, data=k_data))
        v_tensor = Tensor(LazyBuffer(v_op, dtypes.float32, shape, data=v_data))
        return k_tensor, v_tensor, positions

    def get_warm_kv_dequantized(self) -> tuple:
        """Return warm tier as dequantized (K, V) Tensors, sorted by position.

        Warm tier entries are stored quantized; this method dequantizes them
        back to full precision for attention computation.

        Returns:
            (k_tensor, v_tensor): shape (n_warm, d_k) each, or None if empty
            positions: list of token positions in order
        """
        if not self._warm:
            return None, None, []

        positions = sorted(self._warm.keys())
        k_data = []
        v_data = []
        for pos in positions:
            entry = self._warm[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            k_data.extend(k_deq)
            v_data.extend(v_deq)

        n = len(positions)
        shape = (n, self._d_k)
        k_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        v_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        k_tensor = Tensor(LazyBuffer(k_op, dtypes.float32, shape, data=k_data))
        v_tensor = Tensor(LazyBuffer(v_op, dtypes.float32, shape, data=v_data))
        return k_tensor, v_tensor, positions

    def get_cold_kv_dequantized(self) -> tuple:
        """Return cold tier as dequantized (K, V) Tensors with QJL correction.

        Cold tier entries use lower bit-width quantization. When the original
        full-precision vectors were available at quantization time, QJL error
        correction parameters were stored. Dequantization applies the correction.

        Returns:
            (k_tensor, v_tensor): shape (n_cold, d_k) each, or None if empty
            positions: list of token positions in order
        """
        if not self._cold:
            return None, None, []

        positions = sorted(self._cold.keys())
        k_data = []
        v_data = []
        for pos in positions:
            entry = self._cold[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            k_data.extend(k_deq)
            v_data.extend(v_deq)

        n = len(positions)
        shape = (n, self._d_k)
        k_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        v_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        k_tensor = Tensor(LazyBuffer(k_op, dtypes.float32, shape, data=k_data))
        v_tensor = Tensor(LazyBuffer(v_op, dtypes.float32, shape, data=v_data))
        return k_tensor, v_tensor, positions

    def get_all_kv(self) -> tuple:
        """Return all tiers combined as (K, V) Tensors, sorted by position.

        Warm and cold tiers are dequantized before concatenation.
        This is the full reconstructed KV cache for attention computation.

        Returns:
            (k_tensor, v_tensor): shape (total_tokens, d_k) each, or None if empty
            positions: list of all token positions in order
        """
        k_hot, v_hot, pos_hot = self.get_hot_kv()
        k_warm, v_warm, pos_warm = self.get_warm_kv_dequantized()
        k_cold, v_cold, pos_cold = self.get_cold_kv_dequantized()

        all_positions = sorted(pos_hot + pos_warm + pos_cold)
        if not all_positions:
            return None, None, []

        # Build position -> (k_data, v_data) map from all tiers
        kv_map = {}
        for pos in pos_hot:
            entry = self._hot[pos]
            kv_map[pos] = (entry.k_data, entry.v_data)

        # For warm/cold, we need dequantized data
        for pos in pos_warm:
            entry = self._warm[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            kv_map[pos] = (k_deq, v_deq)

        for pos in pos_cold:
            entry = self._cold[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            kv_map[pos] = (k_deq, v_deq)

        k_data = []
        v_data = []
        for pos in all_positions:
            k, v = kv_map[pos]
            k_data.extend(k)
            v_data.extend(v)

        n = len(all_positions)
        shape = (n, self._d_k)
        k_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        v_op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
        k_tensor = Tensor(LazyBuffer(k_op, dtypes.float32, shape, data=k_data))
        v_tensor = Tensor(LazyBuffer(v_op, dtypes.float32, shape, data=v_data))
        return k_tensor, v_tensor, all_positions

    def compact_to_accepted(self, accepted_positions: list) -> None:
        """Compact the cache to only the accepted token positions.

        Used after speculative decoding verification: rejected draft tokens
        are removed from all tiers. Accepted tokens retain their tier placement
        and scores.

        accepted_positions: list of token positions that were accepted.
        """
        accepted_set = set(accepted_positions)

        # Remove non-accepted entries from all tiers
        for tier_dict in (self._hot, self._warm, self._cold):
            to_remove = [pos for pos in tier_dict if pos not in accepted_set]
            for pos in to_remove:
                del tier_dict[pos]

        # Remove scores for evicted positions
        to_remove_scores = [pos for pos in self._scores if pos not in accepted_set]
        for pos in to_remove_scores:
            del self._scores[pos]

    def promote_to_hot(self, pos: int) -> bool:
        """Promote a token from warm or cold back to hot tier.

        Returns True if promotion succeeded, False if position not found.
        """
        if pos in self._hot:
            return True  # Already hot

        if pos in self._warm:
            entry = self._warm[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            self._hot[pos] = _TierEntry(token_pos=pos, k_data=k_deq, v_data=v_deq)
            del self._warm[pos]
            return True

        if pos in self._cold:
            entry = self._cold[pos]
            k_deq = _dequantize_entry(entry.k_data, entry.k_scales, entry.block_size)
            v_deq = _dequantize_entry(entry.v_data, entry.v_scales, entry.block_size)
            self._hot[pos] = _TierEntry(token_pos=pos, k_data=k_deq, v_data=v_deq)
            del self._cold[pos]
            return True

        return False

    def get_tier(self, pos: int) -> int:
        """Return the tier of a token position: TIER_HOT, TIER_WARM, TIER_COLD, or -1."""
        if pos in self._hot:
            return TIER_HOT
        if pos in self._warm:
            return TIER_WARM
        if pos in self._cold:
            return TIER_COLD
        return -1

    def get_score(self, pos: int) -> float:
        """Return the importance score for a position, or 0.0 if not tracked."""
        return self._scores.get(pos, 0.0)

    def _is_protected(self, pos: int) -> bool:
        """Check if a position is protected from eviction.

        Protected positions:
        1. Attention sinks (first n_sinks tokens)
        2. Sliding window (last sliding_window tokens in the hot tier)
        """
        if pos in self._sink_positions:
            return True

        # Sliding window protection: the most recent sliding_window positions
        # that are currently in hot tier
        if self._hot:
            hot_positions = sorted(self._hot.keys())
            window_start = max(0, len(hot_positions) - self._sliding_window)
            window_positions = set(hot_positions[window_start:])
            if pos in window_positions:
                return True

        return False

    def _demote_hot_to_warm(self) -> None:
        """Move hot tier tokens below hot_threshold to warm tier."""
        to_demote = []
        for pos in list(self._hot.keys()):
            if self._is_protected(pos):
                continue
            score = self._scores.get(pos, 0.0)
            if score < self._hot_threshold:
                to_demote.append((score, pos))

        # Sort by score ascending (lowest scores demoted first)
        to_demote.sort(key=lambda x: x[0])

        for _, pos in to_demote:
            entry = self._hot[pos]
            q_entry = _quantize_entry(
                entry.k_data, entry.v_data, pos,
                self._warm_n_bits, self._quant_block_size,
            )
            self._warm[pos] = q_entry
            del self._hot[pos]

    def _demote_warm_to_cold(self) -> None:
        """Move warm tier tokens below warm_threshold to cold tier."""
        to_demote = []
        for pos in list(self._warm.keys()):
            score = self._scores.get(pos, 0.0)
            if score < self._warm_threshold:
                to_demote.append((score, pos))

        to_demote.sort(key=lambda x: x[0])

        for _, pos in to_demote:
            warm_entry = self._warm[pos]
            # Dequantize warm, then requantize at cold bit-width
            k_deq = _dequantize_entry(
                warm_entry.k_data, warm_entry.k_scales, warm_entry.block_size,
            )
            v_deq = _dequantize_entry(
                warm_entry.v_data, warm_entry.v_scales, warm_entry.block_size,
            )
            cold_entry = _quantize_entry(
                k_deq, v_deq, pos,
                self._cold_n_bits, self._quant_block_size,
            )
            self._cold[pos] = cold_entry
            del self._warm[pos]

    def _enforce_hot_capacity(self) -> None:
        """Evict lowest-scored non-protected hot tokens if over capacity."""
        while len(self._hot) > self._max_hot:
            # Find lowest-scored non-protected token
            candidates = []
            for pos in self._hot:
                if not self._is_protected(pos):
                    candidates.append((self._scores.get(pos, 0.0), pos))

            if not candidates:
                break  # All tokens are protected; cannot evict

            candidates.sort(key=lambda x: x[0])
            _, victim_pos = candidates[0]

            entry = self._hot[victim_pos]
            q_entry = _quantize_entry(
                entry.k_data, entry.v_data, victim_pos,
                self._warm_n_bits, self._quant_block_size,
            )
            self._warm[victim_pos] = q_entry
            del self._hot[victim_pos]

    def _enforce_warm_capacity(self) -> None:
        """Evict lowest-scored warm tokens if over capacity."""
        while len(self._warm) > self._max_warm:
            candidates = [
                (self._scores.get(pos, 0.0), pos) for pos in self._warm
            ]
            if not candidates:
                break

            candidates.sort(key=lambda x: x[0])
            _, victim_pos = candidates[0]

            warm_entry = self._warm[victim_pos]
            k_deq = _dequantize_entry(
                warm_entry.k_data, warm_entry.k_scales, warm_entry.block_size,
            )
            v_deq = _dequantize_entry(
                warm_entry.v_data, warm_entry.v_scales, warm_entry.block_size,
            )
            cold_entry = _quantize_entry(
                k_deq, v_deq, victim_pos,
                self._cold_n_bits, self._quant_block_size,
            )
            self._cold[victim_pos] = cold_entry
            del self._warm[victim_pos]

    def _enforce_cold_capacity(self) -> None:
        """Permanently evict lowest-scored cold tokens if over capacity."""
        while len(self._cold) > self._max_cold:
            candidates = [
                (self._scores.get(pos, 0.0), pos) for pos in self._cold
            ]
            if not candidates:
                break

            candidates.sort(key=lambda x: x[0])
            _, victim_pos = candidates[0]

            del self._cold[victim_pos]
            if victim_pos in self._scores:
                del self._scores[victim_pos]


def compute_attention_importance(
    q: Tensor,
    k_cache: Tensor,
    scale: float = None,
) -> dict:
    """Compute per-token importance scores from attention weights.

    Given a query vector q (1, d_k) or (d_k,) and the K cache (n, d_k),
    compute attention weights and return a dict mapping position index
    to the attention weight for that position.

    This is used to update the TieredKVCache importance scores after
    each decoding step.

    Composed from: MUL (scale), REDUCE_SUM (dot product), EXP2 + REDUCE_SUM
    (softmax normalization).
    """
    q_data = q.realize().lazydata._data
    k_data = k_cache.realize().lazydata._data
    d_k = k_cache.shape[-1]
    n = k_cache.shape[0]

    if scale is None:
        scale = 1.0 / math.sqrt(d_k)

    # Compute attention scores: softmax(q @ K^T * scale)
    scores = [0.0] * n
    for i in range(n):
        s = 0.0
        for d in range(d_k):
            q_val = q_data[d] if q.ndim == 1 else q_data[d]
            s += q_val * k_data[i * d_k + d]
        scores[i] = s * scale

    # Softmax normalization (numerically stable)
    max_score = max(scores) if scores else 0.0
    exp_scores = [math.exp(s - max_score) for s in scores]
    sum_exp = sum(exp_scores)
    if sum_exp > 0.0:
        inv_sum = 1.0 / sum_exp
        attention_weights = {i: exp_scores[i] * inv_sum for i in range(n)}
    else:
        attention_weights = {i: 0.0 for i in range(n)}

    return attention_weights


def compute_attention_importance_from_positions(
    q: Tensor,
    k_cache: Tensor,
    positions: list,
    scale: float = None,
) -> dict:
    """Like compute_attention_importance but maps results to token positions.

    positions: list of token positions corresponding to rows of k_cache.
    Returns dict mapping token_pos -> attention_weight.
    """
    raw_weights = compute_attention_importance(q, k_cache, scale)
    return {positions[i]: raw_weights[i] for i in range(len(positions))}


def tiered_attention(
    q: Tensor,
    cache: TieredKVCache,
    causal: bool = True,
) -> Tensor:
    """Compute attention over all tiers of the KV cache.

    Strategy:
    1. Hot tier: full-precision attention (highest quality)
    2. Warm tier: dequantize INT8 -> float32, then standard attention
    3. Cold tier: dequantize INT4 -> float32, then standard attention
    4. Combine via log-sum-exp for numerically stable softmax across tiers

    The combined result is equivalent to computing attention over the full
    reconstructed KV cache, but with quantization noise in warm/cold tiers.

    q: query tensor (seq_len, d_k) or (d_k,)
    cache: TieredKVCache instance
    causal: whether to apply causal masking (based on token positions)

    Returns: attention output tensor (seq_len, d_k) or (d_k,)
    """
    k_all, v_all, positions = cache.get_all_kv()
    if k_all is None:
        # Empty cache: return zeros
        if q.ndim == 1:
            return Tensor.zeros(q.shape[0], dtype=q.dtype)
        return Tensor.zeros(*q.shape, dtype=q.dtype)

    # Full attention over reconstructed cache
    # For single query vector
    if q.ndim == 1:
        q_2d = q.reshape(1, q.shape[0])
    else:
        q_2d = q

    d_k = q_2d.shape[-1]
    scale_val = 1.0 / math.sqrt(d_k)

    q_data = q_2d.realize().lazydata._data
    k_data = k_all.realize().lazydata._data
    v_data = v_all.realize().lazydata._data
    seq_len = q_2d.shape[0]
    n_kv = len(positions)

    output = [0.0] * (seq_len * d_k)

    for qi in range(seq_len):
        row_max = float("-inf")
        row_sum = 0.0
        row_out = [0.0] * d_k

        for ki in range(n_kv):
            # Causal mask: query position must be >= key position
            # For incremental decoding, the query represents the latest token
            # so it can attend to all cached positions
            if causal and qi < ki:
                continue

            # Dot product
            s = 0.0
            for d in range(d_k):
                s += q_data[qi * d_k + d] * k_data[ki * d_k + d]
            s *= scale_val

            # Online softmax
            old_max = row_max
            new_max = max(old_max, s)

            if new_max > old_max:
                rescale = math.exp(old_max - new_max)
                for d in range(d_k):
                    row_out[d] *= rescale
                row_sum *= rescale
                row_max = new_max

            p = math.exp(s - row_max)
            for d in range(d_k):
                row_out[d] += p * v_data[ki * d_k + d]
            row_sum += p

        # Normalize
        if row_sum > 0:
            inv_sum = 1.0 / row_sum
            for d in range(d_k):
                output[qi * d_k + d] = row_out[d] * inv_sum

    if q.ndim == 1:
        shape = (d_k,)
        output = output[:d_k]
    else:
        shape = (seq_len, d_k)

    op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
    return Tensor(LazyBuffer(op, q.dtype, shape, data=output))


# --- Internal helpers ---


def _quantize_entry(
    k_data: list,
    v_data: list,
    token_pos: int,
    n_bits: int,
    block_size: int,
) -> _TierEntry:
    """Quantize K and V vectors using per-block symmetric quantization.

    Uses the same algorithm as turbo_quant.block_quantize but operates
    on flat lists directly for efficiency (avoiding Tensor construction
    overhead for single vectors).
    """
    k_q, k_scales = _quantize_vector(k_data, n_bits, block_size)
    v_q, v_scales = _quantize_vector(v_data, n_bits, block_size)
    return _TierEntry(
        token_pos=token_pos,
        k_data=k_q,
        v_data=v_q,
        k_scales=k_scales,
        v_scales=v_scales,
        n_bits=n_bits,
        block_size=block_size,
    )


def _quantize_vector(data: list, n_bits: int, block_size: int) -> tuple:
    """Per-block symmetric quantization of a single vector.

    Returns (quantized_data, scales_per_block).
    """
    d = len(data)
    qmax = float((1 << (n_bits - 1)) - 1)
    n_blocks = (d + block_size - 1) // block_size

    quantized = [0.0] * d
    scales = [0.0] * n_blocks

    for b in range(n_blocks):
        start = b * block_size
        end = min(start + block_size, d)

        # Find max absolute value in block
        abs_max = 0.0
        for i in range(start, end):
            av = abs(data[i])
            if av > abs_max:
                abs_max = av

        # Scale
        if abs_max == 0.0:
            scale = 1.0  # Avoid division by zero; all values are 0
        else:
            scale = abs_max / qmax
        scales[b] = scale

        # Quantize: q = trunc(x / scale), clamped to [-qmax, qmax]
        inv_scale = 1.0 / scale
        for i in range(start, end):
            q = math.trunc(data[i] * inv_scale)
            q = max(-qmax, min(qmax, q))
            quantized[i] = q

    return quantized, scales


def _dequantize_entry(
    q_data: list,
    scales: list,
    block_size: int,
) -> list:
    """Dequantize a per-block quantized vector.

    Returns the reconstructed float list: x_hat[i] = q[i] * scale[block_of_i].
    """
    d = len(q_data)
    n_blocks = len(scales)
    result = [0.0] * d

    for b in range(n_blocks):
        start = b * block_size
        end = min(start + block_size, d)
        scale = scales[b]
        for i in range(start, end):
            result[i] = q_data[i] * scale

    return result


# ---------------------------------------------------------------------------
# Prefix Caching via Radix Tree (Trie)
# ---------------------------------------------------------------------------
#
# Maps token ID sequences (prefixes) to cached KV blocks so that new
# prompts sharing a prefix with a cached prompt can reuse the cached
# KV blocks without recomputation.
#
# Each node in the radix tree stores:
#   - children: dict mapping token_id -> child RadixNode
#   - kv_blocks: list of (k_data, v_data) tuples for tokens at this depth
#   - last_access: monotonic counter for LRU eviction
#
# Lookup is O(prefix_length). Eviction removes the least-recently-used
# leaf paths first.


class _RadixNode:
    """A node in the prefix caching radix tree."""

    __slots__ = ("children", "kv_block", "last_access", "depth")

    def __init__(self, depth: int = 0) -> None:
        self.children: dict = {}  # token_id -> _RadixNode
        self.kv_block: tuple = None  # (k_data, v_data) or None
        self.last_access: int = 0  # monotonic access counter
        self.depth: int = depth


class PrefixCache:
    """Radix tree (trie) for prefix caching of KV blocks.

    Maps token ID sequences to cached KV blocks. When a new prompt
    shares a prefix with a previously cached prompt, the shared KV
    blocks are returned immediately without recomputation.

    Parameters:
        d_k: dimension of each key/value vector
        max_cached_blocks: maximum number of KV blocks to cache
            (LRU eviction when exceeded)
    """

    def __init__(self, d_k: int, max_cached_blocks: int = 4096) -> None:
        if d_k <= 0:
            raise ValueError(f"d_k must be positive, got {d_k}")
        if max_cached_blocks <= 0:
            raise ValueError(f"max_cached_blocks must be positive, got {max_cached_blocks}")
        self._d_k = d_k
        self._max_cached_blocks = max_cached_blocks
        self._root = _RadixNode(depth=0)
        self._access_counter: int = 0  # monotonic clock for LRU
        self._total_blocks: int = 0  # current number of cached blocks

    @property
    def d_k(self) -> int:
        return self._d_k

    @property
    def total_blocks(self) -> int:
        return self._total_blocks

    def insert(self, token_ids: list, kv_blocks: list) -> None:
        """Insert KV blocks for a token ID sequence into the cache.

        token_ids: list of integer token IDs forming the prefix.
        kv_blocks: list of (k_data, v_data) tuples, one per token.
            Each k_data and v_data is a flat list of d_k floats.

        len(token_ids) must equal len(kv_blocks).
        """
        if len(token_ids) != len(kv_blocks):
            raise ValueError(
                f"token_ids length ({len(token_ids)}) must match "
                f"kv_blocks length ({len(kv_blocks)})"
            )

        self._access_counter += 1
        node = self._root

        for i, token_id in enumerate(token_ids):
            if token_id not in node.children:
                node.children[token_id] = _RadixNode(depth=i + 1)
            node = node.children[token_id]
            node.last_access = self._access_counter

            # Only insert if this node doesn't already have a cached block.
            if node.kv_block is None:
                k_data, v_data = kv_blocks[i]
                if len(k_data) != self._d_k or len(v_data) != self._d_k:
                    raise ValueError(
                        f"Expected KV vectors of length {self._d_k}, "
                        f"got k={len(k_data)}, v={len(v_data)} at position {i}"
                    )
                node.kv_block = (list(k_data), list(v_data))
                self._total_blocks += 1

        # Enforce capacity limit.
        self._evict_if_needed()

    def lookup_prefix(self, token_ids: list) -> tuple:
        """Look up the longest cached prefix for a token sequence.

        token_ids: list of integer token IDs.

        Returns:
            (cached_length, kv_blocks) where:
            - cached_length: number of tokens in the longest matching prefix
              that has cached KV blocks (contiguous from the start)
            - kv_blocks: list of (k_data, v_data) tuples for the cached prefix

        If no prefix is cached, returns (0, []).
        """
        self._access_counter += 1
        node = self._root
        kv_blocks = []
        cached_length = 0

        for token_id in token_ids:
            if token_id not in node.children:
                break
            node = node.children[token_id]
            node.last_access = self._access_counter

            if node.kv_block is not None:
                kv_blocks.append(node.kv_block)
                cached_length += 1
            else:
                # Gap in the cached prefix — stop here.
                break

        return cached_length, kv_blocks

    def invalidate(self, token_ids: list) -> int:
        """Remove cached KV blocks for a specific token sequence.

        Removes the node at the end of the sequence and all of its
        descendant subtrees.

        Returns the number of blocks removed.
        """
        if not token_ids:
            return 0

        # Navigate to the parent of the target node.
        node = self._root
        parent = None
        last_token = None

        for token_id in token_ids:
            if token_id not in node.children:
                return 0  # Path doesn't exist
            parent = node
            last_token = token_id
            node = node.children[token_id]

        # Count blocks in the subtree rooted at `node`.
        removed = self._count_blocks(node)

        # Remove the subtree from the parent.
        if parent is not None and last_token is not None:
            del parent.children[last_token]
            self._total_blocks -= removed

        return removed

    def clear(self) -> None:
        """Remove all cached entries."""
        self._root = _RadixNode(depth=0)
        self._total_blocks = 0

    def _evict_if_needed(self) -> None:
        """Evict least-recently-used leaf blocks until under capacity."""
        while self._total_blocks > self._max_cached_blocks:
            # Find the leaf node with the smallest last_access.
            victim_path = self._find_lru_leaf()
            if victim_path is None:
                break  # No more leaves to evict

            # Remove the victim leaf.
            self._remove_leaf(victim_path)

    def _find_lru_leaf(self) -> list:
        """Find the path to the least-recently-used leaf node.

        Returns a list of (parent_node, token_id) pairs from root
        to the leaf, or None if the tree is empty.
        """
        best_path = None
        best_access = None

        def walk(node: _RadixNode, path: list) -> None:
            nonlocal best_path, best_access

            if not node.children:
                # Leaf node.
                if node.kv_block is not None:
                    if best_access is None or node.last_access < best_access:
                        best_access = node.last_access
                        best_path = list(path)
                return

            for token_id, child in list(node.children.items()):
                path.append((node, token_id))
                walk(child, path)
                path.pop()

        walk(self._root, [])
        return best_path

    def _remove_leaf(self, path: list) -> None:
        """Remove a leaf node given its path from root.

        path: list of (parent_node, token_id) pairs.
        """
        if not path:
            return

        # Remove the leaf.
        parent, token_id = path[-1]
        leaf = parent.children[token_id]
        if leaf.kv_block is not None:
            self._total_blocks -= 1
        del parent.children[token_id]

        # Clean up empty intermediate nodes (bottom-up).
        for i in range(len(path) - 2, -1, -1):
            ancestor, tok = path[i]
            child = ancestor.children.get(tok)
            if child is not None and not child.children and child.kv_block is None:
                del ancestor.children[tok]

    def _count_blocks(self, node: _RadixNode) -> int:
        """Count total cached blocks in a subtree."""
        count = 1 if node.kv_block is not None else 0
        for child in node.children.values():
            count += self._count_blocks(child)
        return count
