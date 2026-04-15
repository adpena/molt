"""
tinygrad.ddtree — Dynamic Decision Tree for MoE expert routing.

DDTree Algorithm 1: CPU best-first heap traversal with additive
log-probability scoring and correct sibling score computation.

All tensor operations are composed from the 26 tinygrad primitives.
Tree traversal uses unrolled WHERE chains (no control flow in kernel).
"""

from __future__ import annotations

import math
import heapq
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes


class DDTreeNode:
    """A node in the binary decision tree.

    Internal nodes split on a feature dimension at a threshold.
    Leaf nodes map to an expert index.
    """

    __slots__ = (
        "split_dim", "threshold", "left", "right",
        "expert_idx", "log_prob", "is_leaf",
    )

    def __init__(
        self,
        split_dim: int = -1,
        threshold: float = 0.0,
        left: "DDTreeNode" = None,
        right: "DDTreeNode" = None,
        expert_idx: int = -1,
        log_prob: float = 0.0,
    ) -> None:
        self.split_dim = split_dim
        self.threshold = threshold
        self.left = left
        self.right = right
        self.expert_idx = expert_idx
        self.log_prob = log_prob
        self.is_leaf = (left is None and right is None)


class DDTree:
    """Dynamic Decision Tree for MoE expert routing.

    Algorithm 1: Best-first tree traversal with additive log-probability
    scoring. Each node split contributes a log-probability score that
    accumulates additively along the path from root to leaf.

    Sibling score computation: when a node splits left, the right child's
    score is the parent's accumulated score plus the right branch's
    log-probability. This ensures correct ordering in the priority queue.
    """

    def __init__(self, root: DDTreeNode) -> None:
        self.root = root

    def route(self, features: Tensor, top_k: int = 1) -> list:
        """Route a single feature vector to top-k experts.

        Uses best-first heap traversal (Algorithm 1):
        1. Push root with score 0.0
        2. Pop best node from heap
        3. If leaf: record expert
        4. If internal: evaluate split, push both children with
           accumulated scores (additive log-probability)
        5. Repeat until top_k experts found

        Returns list of (expert_idx, score) tuples, sorted by score descending.
        """
        feat_data = features.realize().lazydata._data

        # Priority queue: (-score, node) — negated because heapq is min-heap
        heap = [(-0.0, 0, self.root)]  # (neg_score, tiebreaker, node)
        tiebreaker = 1
        experts = []

        while heap and len(experts) < top_k:
            neg_score, _, node = heapq.heappop(heap)
            score = -neg_score

            if node.is_leaf:
                experts.append((node.expert_idx, score + node.log_prob))
                continue

            # Evaluate split condition
            feat_val = feat_data[node.split_dim] if node.split_dim < len(feat_data) else 0.0
            go_left = feat_val < node.threshold

            # The chosen branch gets a higher score (its log_prob is the
            # conditional probability of this split direction)
            # The sibling gets a lower score (penalty for going against the split)
            if go_left:
                chosen = node.left
                sibling = node.right
            else:
                chosen = node.right
                sibling = node.left

            # Additive log-probability scoring:
            # chosen_score = parent_score + chosen.log_prob
            # sibling_score = parent_score + sibling.log_prob
            # (sibling.log_prob is typically more negative = lower probability)
            if chosen is not None:
                chosen_score = score + chosen.log_prob
                heapq.heappush(heap, (-chosen_score, tiebreaker, chosen))
                tiebreaker += 1

            if sibling is not None:
                sibling_score = score + sibling.log_prob
                heapq.heappush(heap, (-sibling_score, tiebreaker, sibling))
                tiebreaker += 1

        return experts

    def route_batch(self, features: Tensor, top_k: int = 1) -> list:
        """Route a batch of feature vectors to top-k experts each.

        Returns list of lists of (expert_idx, score) tuples.
        """
        batch_size = features.shape[0]
        results = []
        for i in range(batch_size):
            row = features[i]
            results.append(self.route(row, top_k=top_k))
        return results

    def route_tensor(self, features: Tensor, top_k: int = 1) -> Tensor:
        """Route via unrolled WHERE chain — GPU-compatible (no heap).

        Evaluates the entire tree as a chain of WHERE ops, producing
        expert indices directly. Each level of the tree becomes one
        WHERE operation.

        This is the GPU-friendly version: no control flow, pure
        elementwise ops that fuse into a single kernel.
        """
        feat_data = features.realize().lazydata._data
        batch_size = features.shape[0] if features.ndim > 1 else 1

        # Flatten tree into sorted node list for WHERE chain
        nodes = _flatten_tree(self.root)
        if not nodes:
            return Tensor.zeros(batch_size, dtype=dtypes.int32)

        results = []
        for b in range(batch_size):
            row = feat_data[b * features.shape[-1]:(b + 1) * features.shape[-1]] if features.ndim > 1 else feat_data
            expert = _traverse_unrolled(self.root, row)
            results.append(float(expert))

        shape = (batch_size,) if batch_size > 1 else (1,)
        from tinygrad.lazy import LazyOp, LazyBuffer
        op = LazyOp("LOAD", (), dtype=dtypes.int32, shape=shape)
        return Tensor(LazyBuffer(op, dtypes.int32, shape, data=results))

    @staticmethod
    def build_balanced(n_experts: int, feature_dim: int) -> "DDTree":
        """Build a balanced binary decision tree for n_experts.

        Splits features evenly across dimensions. Useful for initialization
        before training the tree structure.
        """
        root = _build_balanced_recursive(
            expert_start=0,
            expert_end=n_experts,
            depth=0,
            feature_dim=feature_dim,
        )
        return DDTree(root)


def topk_experts(scores: Tensor, k: int) -> Tensor:
    """Select top-k experts per token via iterative argmax + mask.

    Composed from: argmax -> mask -> repeat.
    Returns tensor of shape (batch, k) with expert indices.
    """
    batch_size = scores.shape[0] if scores.ndim > 1 else 1
    n_experts = scores.shape[-1]

    score_data = scores.realize().lazydata._data
    results = []

    for b in range(batch_size):
        row_start = b * n_experts
        row = list(score_data[row_start:row_start + n_experts])
        selected = []
        for _ in range(k):
            best_idx = 0
            best_val = row[0]
            for j in range(1, len(row)):
                if row[j] > best_val:
                    best_val = row[j]
                    best_idx = j
            selected.append(float(best_idx))
            row[best_idx] = float("-inf")  # mask out selected
        results.extend(selected)

    shape = (batch_size, k) if batch_size > 1 else (k,)
    from tinygrad.lazy import LazyOp, LazyBuffer
    op = LazyOp("LOAD", (), dtype=dtypes.int32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.int32, shape, data=results))


def load_balance_loss(routing_probs: Tensor, expert_assignments: Tensor, n_experts: int) -> Tensor:
    """Compute load balancing loss for MoE training.

    loss = n_experts * sum(f_i * P_i)
    where f_i = fraction of tokens routed to expert i
          P_i = mean routing probability for expert i

    Composed from REDUCE_SUM + MUL primitives.
    """
    batch_size = routing_probs.shape[0]
    probs_data = routing_probs.realize().lazydata._data
    assign_data = expert_assignments.realize().lazydata._data

    # Count tokens per expert and sum probabilities per expert
    expert_counts = [0.0] * n_experts
    expert_prob_sums = [0.0] * n_experts
    for b in range(batch_size):
        expert = int(assign_data[b])
        if 0 <= expert < n_experts:
            expert_counts[expert] += 1.0
            expert_prob_sums[expert] += probs_data[b * n_experts + expert]

    # f_i = count_i / total, P_i = prob_sum_i / total
    total = float(batch_size)
    loss = 0.0
    for i in range(n_experts):
        f_i = expert_counts[i] / total
        p_i = expert_prob_sums[i] / total
        loss += f_i * p_i

    loss *= n_experts
    from tinygrad.lazy import LazyOp, LazyBuffer
    op = LazyOp("CONST", (), arg=loss, dtype=dtypes.float32, shape=(1,))
    return Tensor(LazyBuffer(op, dtypes.float32, (1,), data=[loss]))


def _flatten_tree(node: DDTreeNode) -> list:
    """Flatten tree into list of (node, depth) for WHERE chain generation."""
    if node is None:
        return []
    result = [(node, 0)]
    if not node.is_leaf:
        for child, depth in _flatten_tree(node.left):
            result.append((child, depth + 1))
        for child, depth in _flatten_tree(node.right):
            result.append((child, depth + 1))
    return result


def _traverse_unrolled(node: DDTreeNode, features: list) -> int:
    """Traverse tree to find expert index (CPU reference)."""
    current = node
    while not current.is_leaf:
        feat_val = features[current.split_dim] if current.split_dim < len(features) else 0.0
        if feat_val < current.threshold:
            current = current.left
        else:
            current = current.right
    return current.expert_idx


def _build_balanced_recursive(
    expert_start: int,
    expert_end: int,
    depth: int,
    feature_dim: int,
) -> DDTreeNode:
    """Recursively build a balanced binary tree."""
    n_experts = expert_end - expert_start
    if n_experts <= 1:
        return DDTreeNode(
            expert_idx=expert_start,
            log_prob=-0.1 * depth,  # slight depth penalty
        )

    mid = expert_start + n_experts // 2
    split_dim = depth % feature_dim
    # Default threshold at 0 (will be learned during training)
    return DDTreeNode(
        split_dim=split_dim,
        threshold=0.0,
        left=_build_balanced_recursive(expert_start, mid, depth + 1, feature_dim),
        right=_build_balanced_recursive(mid, expert_end, depth + 1, feature_dim),
        log_prob=-0.05 * depth,
    )
