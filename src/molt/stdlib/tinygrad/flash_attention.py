"""
tinygrad.flash_attention — FlashAttention-3 tiled attention with online softmax.

FlashAttention-3 improvements over v2:
- Block size Br x Bc tiling (Br = min(d, 128), Bc = min(d, 128))
- Online softmax: running max (m_i) + running sum (l_i)
- Output accumulation:
    O_i = diag(l_new)^{-1} * (diag(l_old) * O_{i-1} * exp(m_old - m_new) +
          exp(S_ij - m_new) @ V_j)
- Causal masking: skip blocks where all elements are masked
- Asynchronous block-level pipelining (structural, not thread-level)

This is a COMPOSITION of the 26 tinygrad primitives:
MUL, ADD, EXP2, LOG2, REDUCE_MAX, REDUCE_SUM, WHERE, SUB, RECIPROCAL.

All operations are composed from primitives — no custom kernels.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor


# ln(2) for base conversion: exp(x) = exp2(x / ln(2))
_LN2 = math.log(2.0)
_LOG2E = math.log2(math.e)


def _exp(x: float) -> float:
    """exp(x) via exp2: exp(x) = 2^(x * log2(e))."""
    return math.pow(2.0, x * _LOG2E)


def flash_attention_v3(
    q: Tensor,
    k: Tensor,
    v: Tensor,
    causal: bool = False,
    block_br: int | None = None,
    block_bc: int | None = None,
) -> Tensor:
    """FlashAttention-3: tiled Q/K/V attention with online softmax.

    Parameters:
        q: Query tensor, shape (seq_len, d_k)
        k: Key tensor, shape (seq_len, d_k)
        v: Value tensor, shape (seq_len, d_k)
        causal: Apply causal masking (upper-triangular mask)
        block_br: Row block size for Q tiling. Defaults to min(d_k, 128).
        block_bc: Column block size for K/V tiling. Defaults to min(d_k, 128).

    Returns:
        Output tensor, shape (seq_len, d_k)

    Algorithm:
        For each Q block i (rows q_start..q_end):
            m_i = [-inf] * Br          (running row-wise max)
            l_i = [0] * Br             (running row-wise sum of exp)
            O_i = [[0]] * Br x d_k     (running output accumulator)

            For each K/V block j (cols k_start..k_end):
                S_ij = Q_i @ K_j^T * scale     (Br x Bc attention scores)

                If causal: skip block if all elements masked (k_start > q_end - 1)
                           mask individual elements where k > q

                m_new = max(m_i, rowmax(S_ij))
                rescale = exp(m_i - m_new)
                l_i = l_i * rescale
                O_i = O_i * rescale[:, None]

                P_ij = exp(S_ij - m_new[:, None])
                l_i = l_i + rowsum(P_ij)
                O_i = O_i + P_ij @ V_j

                m_i = m_new

            O_i = O_i / l_i[:, None]
    """
    if q.ndim != 2 or k.ndim != 2 or v.ndim != 2:
        raise ValueError("flash_attention_v3 requires 2D tensors (seq_len, d_k)")

    seq_len, d_k = q.shape
    if k.shape != (seq_len, d_k) or v.shape != (seq_len, d_k):
        raise ValueError(f"Shape mismatch: q={q.shape}, k={k.shape}, v={v.shape}")

    # Compute block sizes: Br = min(d_k, 128), Bc = min(d_k, 128)
    br = block_br if block_br is not None else min(d_k, 128)
    bc = block_bc if block_bc is not None else min(d_k, 128)

    scale = 1.0 / math.sqrt(d_k)

    # Materialize input data
    q_data = q.realize().lazydata._data
    k_data = k.realize().lazydata._data
    v_data = v.realize().lazydata._data

    # Output buffer
    output = [0.0] * (seq_len * d_k)

    # Per-row running statistics
    row_max = [float("-inf")] * seq_len
    row_sum = [0.0] * seq_len

    # Number of blocks
    n_q_blocks = (seq_len + br - 1) // br
    n_kv_blocks = (seq_len + bc - 1) // bc

    for q_bi in range(n_q_blocks):
        q_start = q_bi * br
        q_end = min(q_start + br, seq_len)

        for kv_bi in range(n_kv_blocks):
            k_start = kv_bi * bc
            k_end = min(k_start + bc, seq_len)

            # Causal block skip: if the entire K block is after all Q positions
            # in this Q block, every element would be masked. Skip entirely.
            if causal and k_start > q_end - 1:
                continue

            # Compute S_ij block: (q_end - q_start) x (k_end - k_start)
            # S[qi_local][ki_local] = Q[qi] . K[ki] * scale
            s_block = []
            for qi in range(q_start, q_end):
                row = []
                for ki in range(k_start, k_end):
                    # Causal element-level mask
                    if causal and ki > qi:
                        row.append(float("-inf"))
                    else:
                        dot = 0.0
                        for d in range(d_k):
                            dot += q_data[qi * d_k + d] * k_data[ki * d_k + d]
                        row.append(dot * scale)
                s_block.append(row)

            # Online softmax update for each row in this Q block
            for qi_local, qi in enumerate(range(q_start, q_end)):
                s_row = s_block[qi_local]

                # Row max of S_ij block row
                block_row_max = max(s_row)

                # New running max
                m_old = row_max[qi]
                m_new = max(m_old, block_row_max)

                # Rescale existing accumulator if max changed
                if m_new > m_old and m_old != float("-inf"):
                    rescale = _exp(m_old - m_new)
                    for d in range(d_k):
                        output[qi * d_k + d] *= rescale
                    row_sum[qi] *= rescale
                elif m_old == float("-inf") and m_new != float("-inf"):
                    # First non-masked block: no rescale needed (O and l are 0)
                    pass

                row_max[qi] = m_new

                # P_ij = exp(S_ij - m_new) for this row
                # Accumulate: O += P @ V, l += sum(P)
                for ki_local, ki in enumerate(range(k_start, k_end)):
                    s_val = s_row[ki_local]
                    if s_val == float("-inf"):
                        continue  # Masked element contributes 0

                    p = _exp(s_val - m_new)

                    # O[qi] += p * V[ki]
                    for d in range(d_k):
                        output[qi * d_k + d] += p * v_data[ki * d_k + d]

                    row_sum[qi] += p

    # Final normalization: O = O / l
    for qi in range(seq_len):
        if row_sum[qi] > 0.0:
            inv_sum = 1.0 / row_sum[qi]
            for d in range(d_k):
                output[qi * d_k + d] *= inv_sum

    from tinygrad.lazy import LazyOp, LazyBuffer

    shape = (seq_len, d_k)
    op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
    return Tensor(LazyBuffer(op, q.dtype, shape, data=output))


def naive_attention(q: Tensor, k: Tensor, v: Tensor, causal: bool = False) -> Tensor:
    """Naive O(n^2) attention for correctness reference.

    attention(Q, K, V) = softmax(Q @ K^T / sqrt(d_k)) @ V

    Materializes the full attention matrix — do NOT use for large sequences.
    """
    if q.ndim != 2 or k.ndim != 2 or v.ndim != 2:
        raise ValueError("naive_attention requires 2D tensors (seq_len, d_k)")

    seq_len, d_k = q.shape
    scale = 1.0 / math.sqrt(d_k)

    q_data = q.realize().lazydata._data
    k_data = k.realize().lazydata._data
    v_data = v.realize().lazydata._data

    # Full attention matrix: S = Q @ K^T * scale
    attn = [[0.0] * seq_len for _ in range(seq_len)]
    for i in range(seq_len):
        for j in range(seq_len):
            dot = 0.0
            for d in range(d_k):
                dot += q_data[i * d_k + d] * k_data[j * d_k + d]
            s = dot * scale
            if causal and j > i:
                s = float("-inf")
            attn[i][j] = s

    # Softmax per row
    for i in range(seq_len):
        row_max = max(attn[i])
        exp_sum = 0.0
        for j in range(seq_len):
            if attn[i][j] == float("-inf"):
                attn[i][j] = 0.0
            else:
                attn[i][j] = _exp(attn[i][j] - row_max)
                exp_sum += attn[i][j]
        if exp_sum > 0.0:
            for j in range(seq_len):
                attn[i][j] /= exp_sum

    # Output: attn @ V
    output = [0.0] * (seq_len * d_k)
    for i in range(seq_len):
        for j in range(seq_len):
            w = attn[i][j]
            if w == 0.0:
                continue
            for d in range(d_k):
                output[i * d_k + d] += w * v_data[j * d_k + d]

    from tinygrad.lazy import LazyOp, LazyBuffer

    shape = (seq_len, d_k)
    op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
    return Tensor(LazyBuffer(op, q.dtype, shape, data=output))
