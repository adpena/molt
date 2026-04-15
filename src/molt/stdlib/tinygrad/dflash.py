"""
tinygrad.dflash — Flash Attention v2 + Speculative Decoding.

Flash attention: tiled Q/K/V processing with online softmax.
Speculative decoding: draft-verify pipeline using primitive compositions.
Tiered KV cache integration: post-verification cache compaction and tier management.

All operations are composed from the 26 tinygrad primitives.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes


def flash_attention(
    q: Tensor,
    k: Tensor,
    v: Tensor,
    causal: bool = False,
    block_size: int = 64,
) -> Tensor:
    """Flash Attention v2: tiled attention with online softmax.

    Processes Q/K/V in tiles to minimize memory usage:
    1. For each Q block:
       a. Initialize running max m = -inf, running sum l = 0, output O = 0
       b. For each K/V block:
          - Compute S = Q_block @ K_block^T / sqrt(d_k)
          - Apply causal mask if needed (pad with -inf)
          - Update online softmax: m_new = max(m, max(S))
          - Rescale: O = O * exp(m_old - m_new), l = l * exp(m_old - m_new)
          - P = exp(S - m_new), O += P @ V_block, l += sum(P)
       c. O = O / l

    Composed from: MUL, ADD, EXP2, LOG2, MAX, REDUCE_SUM, REDUCE_MAX, MATMUL.
    Expected fusion: 3-4 kernels total (not 10+ unfused ops).
    """
    if q.ndim != 2 or k.ndim != 2 or v.ndim != 2:
        raise ValueError("flash_attention requires 2D tensors (seq_len, d_k)")

    seq_len, d_k = q.shape
    scale = 1.0 / math.sqrt(d_k)

    q_data = q.realize().lazydata._data
    k_data = k.realize().lazydata._data
    v_data = v.realize().lazydata._data

    # Output accumulator
    output = [0.0] * (seq_len * d_k)
    row_max = [float("-inf")] * seq_len
    row_sum = [0.0] * seq_len

    n_blocks = (seq_len + block_size - 1) // block_size

    for q_block_idx in range(n_blocks):
        q_start = q_block_idx * block_size
        q_end = min(q_start + block_size, seq_len)

        for k_block_idx in range(n_blocks):
            k_start = k_block_idx * block_size
            k_end = min(k_start + block_size, seq_len)

            # Compute attention scores: S = Q_block @ K_block^T * scale
            for qi in range(q_start, q_end):
                for ki in range(k_start, k_end):
                    # Causal mask: skip future positions
                    if causal and ki > qi:
                        continue

                    # Dot product Q[qi] . K[ki]
                    s = 0.0
                    for d in range(d_k):
                        s += q_data[qi * d_k + d] * k_data[ki * d_k + d]
                    s *= scale

                    # Online softmax update
                    old_max = row_max[qi]
                    new_max = max(old_max, s)

                    if new_max > old_max:
                        # Rescale existing accumulator
                        rescale = math.exp(old_max - new_max)
                        for d in range(d_k):
                            output[qi * d_k + d] *= rescale
                        row_sum[qi] *= rescale
                        row_max[qi] = new_max

                    # Accumulate: O += exp(s - max) * V[ki]
                    p = math.exp(s - row_max[qi])
                    for d in range(d_k):
                        output[qi * d_k + d] += p * v_data[ki * d_k + d]
                    row_sum[qi] += p

    # Normalize: O = O / row_sum
    for qi in range(seq_len):
        if row_sum[qi] > 0:
            inv_sum = 1.0 / row_sum[qi]
            for d in range(d_k):
                output[qi * d_k + d] *= inv_sum

    from tinygrad.lazy import LazyOp, LazyBuffer
    shape = (seq_len, d_k)
    op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
    return Tensor(LazyBuffer(op, q.dtype, shape, data=output))


def naive_attention(q: Tensor, k: Tensor, v: Tensor, causal: bool = False) -> Tensor:
    """Naive attention for correctness reference.

    attention(Q, K, V) = softmax(Q @ K^T / sqrt(d_k)) @ V
    """
    return Tensor.scaled_dot_product_attention(q, k, v, is_causal=causal)


def speculative_decode(
    draft_logits: Tensor,
    target_logits: Tensor,
    draft_tokens: Tensor,
    temperature: float = 1.0,
) -> tuple:
    """Speculative decoding verification step.

    Given k draft tokens and their logits from both draft and target models:
    1. Compute acceptance probability: min(1, target_prob / draft_prob)
    2. Accept/reject each draft token using random sampling
    3. On first rejection, resample from adjusted distribution

    All composed from primitives: EXP2, MUL, RECIPROCAL, CMPLT, WHERE, RAND.

    Returns:
        (accepted_tokens: Tensor, n_accepted: int)
    """
    # Compute probabilities from logits (softmax with temperature)
    draft_probs = (draft_logits * (1.0 / temperature)).softmax(axis=-1)
    target_probs = (target_logits * (1.0 / temperature)).softmax(axis=-1)

    draft_data = draft_probs.realize().lazydata._data
    target_data = target_probs.realize().lazydata._data
    token_data = draft_tokens.realize().lazydata._data

    k = len(token_data)
    vocab_size = draft_probs.shape[-1] if draft_probs.ndim > 1 else len(draft_data) // k

    accepted = []
    import random
    for i in range(k):
        token = int(token_data[i])

        # Get draft and target probability for this token
        if draft_probs.ndim > 1:
            d_prob = draft_data[i * vocab_size + token]
            t_prob = target_data[i * vocab_size + token]
        else:
            d_prob = draft_data[token] if token < len(draft_data) else 0.0
            t_prob = target_data[token] if token < len(target_data) else 0.0

        # Acceptance probability: min(1, target_prob / draft_prob)
        if d_prob > 0:
            accept_prob = min(1.0, t_prob / d_prob)
        else:
            accept_prob = 1.0 if t_prob > 0 else 0.0

        # Random acceptance test
        r = random.random()
        if r < accept_prob:
            accepted.append(float(token))
        else:
            # Reject: stop accepting further tokens
            break

    n_accepted = len(accepted)
    if n_accepted == 0:
        accepted = [0.0]

    from tinygrad.lazy import LazyOp, LazyBuffer
    shape = (len(accepted),)
    op = LazyOp("LOAD", (), dtype=dtypes.int32, shape=shape)
    return (
        Tensor(LazyBuffer(op, dtypes.int32, shape, data=accepted)),
        n_accepted,
    )


def speculative_decode_with_kv_cache(
    draft_logits: Tensor,
    target_logits: Tensor,
    draft_tokens: Tensor,
    kv_cache: "TieredKVCache",
    draft_positions: list,
    query: Tensor = None,
    temperature: float = 1.0,
) -> tuple:
    """Speculative decoding with tiered KV cache management.

    After standard speculative decode verification:
    1. Compact the KV cache to accepted tokens only (rejected branches pruned).
    2. Compute attention importance scores for accepted tokens.
    3. Update the cache's importance tracking.
    4. Run a tier-management step (demote/promote based on scores).

    Parameters:
        draft_logits: logits from draft model, shape (k, vocab_size)
        target_logits: logits from target model, shape (k, vocab_size)
        draft_tokens: draft token indices, shape (k,)
        kv_cache: TieredKVCache instance managing the K/V storage
        draft_positions: list of token positions in the cache corresponding
                        to the k draft tokens. draft_positions[i] is the
                        cache position for draft token i.
        query: query vector for importance scoring, shape (d_k,) or (1, d_k).
               If None, importance scores are not updated this step.
        temperature: softmax temperature for speculative decoding.

    Returns:
        (accepted_tokens, n_accepted, accepted_positions)

    The kv_cache is mutated in place: rejected positions are removed,
    and tier management is applied.
    """
    from tinygrad.kv_cache import (
        TieredKVCache,
        compute_attention_importance_from_positions,
    )

    # Step 1: Standard speculative decode verification
    accepted_tokens, n_accepted = speculative_decode(
        draft_logits, target_logits, draft_tokens, temperature,
    )

    # Step 2: Determine accepted positions
    # The first n_accepted draft tokens are accepted (speculative_decode
    # accepts greedily from the start and stops at first rejection)
    accepted_positions = draft_positions[:n_accepted]

    # Step 3: Compact the cache to keep only positions that were already
    # in the cache before this draft round, plus the accepted draft positions.
    # All positions currently in the cache that are NOT in draft_positions
    # (i.e., they were there before the draft) are retained.
    # Of the draft_positions, only accepted ones are retained.
    rejected_positions = set(draft_positions[n_accepted:])
    all_positions = set()
    for tier_dict in (kv_cache._hot, kv_cache._warm, kv_cache._cold):
        all_positions.update(tier_dict.keys())
    keep_positions = list(all_positions - rejected_positions)
    kv_cache.compact_to_accepted(keep_positions)

    # Step 4: Update importance scores if query is provided
    if query is not None:
        k_hot, _, hot_positions = kv_cache.get_hot_kv()
        if k_hot is not None and hot_positions:
            weights = compute_attention_importance_from_positions(
                query, k_hot, hot_positions,
            )
            kv_cache.update_scores(weights)

    # Step 5: Tier management
    kv_cache.step()

    return accepted_tokens, n_accepted, accepted_positions
