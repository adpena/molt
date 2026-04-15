"""
tinygrad.tree_attention — Ancestor-only attention mask + KV cache compaction.

For speculative decoding tree verification: only attend to ancestor tokens
in the draft tree, not siblings. This enables correct parallel verification
of multiple candidate continuations.

Tiered KV cache integration: attention over hot/warm/cold tiers with
automatic dequantization for warm and cold entries.

All operations composed from the 26 tinygrad primitives.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes


def build_ancestor_mask(tree_structure: list) -> Tensor:
    """Build an ancestor-only attention mask from a tree structure.

    tree_structure: list of parent indices. tree_structure[i] = parent of node i.
                    Root node has parent = -1.

    Returns: (n, n) boolean mask where mask[i][j] = 1 iff j is an ancestor of i
             (or j == i). This ensures each token only attends to its ancestors
             in the draft tree, not its siblings.

    Example:
        tree = [-1, 0, 0, 1, 1, 2]
        # Node 0 is root (parent=-1)
        # Nodes 1,2 are children of 0
        # Nodes 3,4 are children of 1
        # Node 5 is child of 2
        mask = build_ancestor_mask(tree)
        # mask[3] = [1, 1, 0, 1, 0, 0]  (3 attends to 0, 1, 3)
        # mask[5] = [1, 0, 1, 0, 0, 1]  (5 attends to 0, 2, 5)
    """
    n = len(tree_structure)
    mask_data = [0.0] * (n * n)

    for i in range(n):
        # Self-attention: always attend to self
        mask_data[i * n + i] = 1.0

        # Walk up to root, marking all ancestors
        current = tree_structure[i]
        while current >= 0:
            mask_data[i * n + current] = 1.0
            current = tree_structure[current]

    from tinygrad.lazy import LazyOp, LazyBuffer
    shape = (n, n)
    op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.float32, shape, data=mask_data))


def tree_attention(
    q: Tensor,
    k: Tensor,
    v: Tensor,
    tree_structure: list,
) -> Tensor:
    """Attention with ancestor-only masking for tree-structured decoding.

    Uses the ancestor mask to ensure each position only attends to its
    ancestors in the draft tree, preventing information leakage between
    sibling branches.

    Composed from: MATMUL, MUL, ADD (mask), SOFTMAX, MATMUL.
    """
    d_k = q.shape[-1]
    scale = 1.0 / math.sqrt(d_k)

    # Build ancestor mask
    mask = build_ancestor_mask(tree_structure)

    # Convert mask: 0 -> -inf, 1 -> 0 (for additive masking)
    # neg_inf_mask = (1 - mask) * (-inf)
    one = Tensor._const(1.0, mask.shape, mask.dtype)
    neg_inf = Tensor._const(float("-inf"), mask.shape, mask.dtype)
    additive_mask = (one - mask) * neg_inf

    # Scaled dot-product attention with mask
    return Tensor.scaled_dot_product_attention(q, k, v, attn_mask=additive_mask)


def compact_kv_cache(
    k_cache: Tensor,
    v_cache: Tensor,
    accepted_indices: list,
) -> tuple:
    """Compact KV cache by keeping only accepted token positions.

    After speculative decoding verification, rejected branches are pruned.
    This function extracts only the accepted KV entries to avoid wasting
    memory on rejected draft tokens.

    Returns (compacted_k, compacted_v) with only accepted positions.
    """
    k_data = k_cache.realize().lazydata._data
    v_data = v_cache.realize().lazydata._data
    d_k = k_cache.shape[-1]

    n_accepted = len(accepted_indices)
    new_k_data = []
    new_v_data = []

    for idx in accepted_indices:
        start = idx * d_k
        end = start + d_k
        new_k_data.extend(k_data[start:end])
        new_v_data.extend(v_data[start:end])

    new_shape = (n_accepted, d_k)
    from tinygrad.lazy import LazyOp, LazyBuffer
    k_op = LazyOp("LOAD", (), dtype=k_cache.dtype, shape=new_shape)
    v_op = LazyOp("LOAD", (), dtype=v_cache.dtype, shape=new_shape)
    return (
        Tensor(LazyBuffer(k_op, k_cache.dtype, new_shape, data=new_k_data)),
        Tensor(LazyBuffer(v_op, v_cache.dtype, new_shape, data=new_v_data)),
    )


def tiered_tree_attention(
    q: Tensor,
    kv_cache: "TieredKVCache",
    tree_structure: list,
    tree_positions: list,
) -> Tensor:
    """Tree attention over a tiered KV cache.

    Combines ancestor-only masking with tiered KV cache retrieval:
    1. Retrieve hot tier KV (full precision) - standard attention
    2. Retrieve warm tier KV (dequantized from INT8) - attention with
       quantization noise but still high quality
    3. Retrieve cold tier KV (dequantized from INT4) - attention with
       higher noise for distant context
    4. Apply ancestor mask to the combined KV cache
    5. Compute attention with online softmax across all tiers

    Parameters:
        q: query tensor, shape (n_tree_nodes, d_k)
        kv_cache: TieredKVCache instance
        tree_structure: list of parent indices for tree nodes.
                       tree_structure[i] = parent of node i, root = -1.
        tree_positions: list of cache positions for each tree node.
                       tree_positions[i] = the cache position that node i
                       should attend to (mapping tree node -> cache token).

    Returns:
        output: attention output, shape (n_tree_nodes, d_k)

    The attention for each tree node i attends to:
    - Its own KV at tree_positions[i]
    - Its ancestors' KVs at tree_positions[ancestor_j] for each ancestor j
    - All non-tree (prefix) tokens in the cache

    Prefix tokens (those in the cache but not in tree_positions) are attended
    to by all tree nodes (they are context, not part of the speculative tree).
    """
    from tinygrad.kv_cache import TieredKVCache

    n_nodes = len(tree_structure)
    d_k = q.shape[-1]
    scale = 1.0 / math.sqrt(d_k)

    # Get all KV data from the tiered cache
    k_all, v_all, all_positions = kv_cache.get_all_kv()
    if k_all is None:
        shape = (n_nodes, d_k)
        from tinygrad.lazy import LazyOp, LazyBuffer
        op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
        return Tensor(LazyBuffer(op, q.dtype, shape, data=[0.0] * (n_nodes * d_k)))

    q_data = q.realize().lazydata._data
    k_data = k_all.realize().lazydata._data
    v_data = v_all.realize().lazydata._data
    n_kv = len(all_positions)

    # Build position -> index map for the full KV cache
    pos_to_kv_idx = {pos: idx for idx, pos in enumerate(all_positions)}

    # Build set of tree positions for ancestor masking
    tree_pos_set = set(tree_positions)

    # Build ancestor sets for each tree node
    # ancestors[i] = set of positions that node i can attend to
    ancestors = []
    for i in range(n_nodes):
        anc = {tree_positions[i]}  # always attend to self
        current = tree_structure[i]
        while current >= 0:
            anc.add(tree_positions[current])
            current = tree_structure[current]
        ancestors.append(anc)

    # Compute attention with ancestor masking
    output = [0.0] * (n_nodes * d_k)

    for qi in range(n_nodes):
        row_max = float("-inf")
        row_sum = 0.0
        row_out = [0.0] * d_k

        for ki in range(n_kv):
            kv_pos = all_positions[ki]

            # Masking logic:
            # - Non-tree (prefix) positions: always attend
            # - Tree positions: only attend if ancestor of qi
            if kv_pos in tree_pos_set and kv_pos not in ancestors[qi]:
                continue

            # Dot product
            s = 0.0
            for d in range(d_k):
                s += q_data[qi * d_k + d] * k_data[ki * d_k + d]
            s *= scale

            # Online softmax update
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

    shape = (n_nodes, d_k)
    from tinygrad.lazy import LazyOp, LazyBuffer
    op = LazyOp("LOAD", (), dtype=q.dtype, shape=shape)
    return Tensor(LazyBuffer(op, q.dtype, shape, data=output))


def compact_tiered_kv_cache(
    kv_cache: "TieredKVCache",
    accepted_indices: list,
    tree_positions: list,
    query: Tensor = None,
) -> None:
    """Compact a tiered KV cache after tree verification.

    After tree-structured speculative decoding verification:
    1. Map accepted tree node indices to cache positions
    2. Remove rejected tree positions from the cache
    3. Update importance scores if query is provided
    4. Run tier management step

    Parameters:
        kv_cache: TieredKVCache instance (mutated in place)
        accepted_indices: list of accepted tree node indices (0-based)
        tree_positions: list mapping tree node index -> cache position
        query: query vector for importance scoring (optional)
    """
    from tinygrad.kv_cache import compute_attention_importance_from_positions

    # Determine which tree positions are accepted vs rejected
    accepted_tree_positions = set(tree_positions[i] for i in accepted_indices)
    all_tree_positions = set(tree_positions)
    rejected_positions = all_tree_positions - accepted_tree_positions

    # Build the set of all positions to keep: everything except rejected
    all_positions = set()
    for tier_dict in (kv_cache._hot, kv_cache._warm, kv_cache._cold):
        all_positions.update(tier_dict.keys())
    keep_positions = list(all_positions - rejected_positions)
    kv_cache.compact_to_accepted(keep_positions)

    # Update importance scores
    if query is not None:
        k_hot, _, hot_positions = kv_cache.get_hot_kv()
        if k_hot is not None and hot_positions:
            weights = compute_attention_importance_from_positions(
                query, k_hot, hot_positions,
            )
            kv_cache.update_scores(weights)

    # Tier management
    kv_cache.step()
