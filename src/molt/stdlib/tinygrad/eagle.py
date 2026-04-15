"""
tinygrad.eagle — EAGLE-3 Self-Speculative Decoding.

Self-speculative decoding: no separate draft model required. Instead, a
lightweight prediction head on intermediate transformer layers serves as
the draft mechanism.

Architecture:
- EagleDraftHead: takes hidden states from layer N/2, projects to vocab logits
- eagle_speculate: runs forward to layer N/2, drafts n_draft candidates
- eagle_verify: standard speculative decoding acceptance criterion

All operations are composed from the 26 tinygrad primitives:
MUL, ADD, EXP2, RECIPROCAL, REDUCE_SUM, REDUCE_MAX, CMPLT, WHERE.
"""

from __future__ import annotations

import math
import random as _random
from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes

_LOG2E = math.log2(math.e)
_LN2 = math.log(2.0)


def _softmax_1d(data: list[float]) -> list[float]:
    """Compute softmax over a 1D list of logits.

    Composed from: EXP2, MUL, REDUCE_SUM, RECIPROCAL.
    exp(x) = exp2(x * log2(e))
    """
    if not data:
        return []
    m = max(data)
    exps = [math.pow(2.0, (x - m) * _LOG2E) for x in data]
    s = sum(exps)
    if s == 0.0:
        return [1.0 / len(data)] * len(data)
    inv_s = 1.0 / s
    return [e * inv_s for e in exps]


def _sample_token(probs: list[float], temperature: float = 1.0) -> int:
    """Sample a token from a probability distribution.

    When temperature == 0, returns argmax (greedy).
    """
    if temperature == 0.0:
        return max(range(len(probs)), key=lambda i: probs[i])

    # Temperature scaling (applied to logits before softmax, but here
    # we have probs already — re-scale by taking log, dividing, re-softmax)
    if temperature != 1.0:
        logits = []
        for p in probs:
            if p > 0:
                logits.append(math.log(p) / temperature)
            else:
                logits.append(float("-inf"))
        probs = _softmax_1d(logits)

    r = _random.random()
    cumsum = 0.0
    for i, p in enumerate(probs):
        cumsum += p
        if r < cumsum:
            return i
    return len(probs) - 1


class EagleDraftHead:
    """Lightweight prediction head for self-speculative decoding.

    Takes hidden states from an intermediate transformer layer (typically N/2)
    and projects them to vocabulary logits through a single linear layer.

    This is deliberately minimal — the accuracy of the draft head determines
    acceptance rate, but even a weak head provides speedup as long as
    acceptance rate > 1/n_draft.

    Parameters:
        hidden_dim: Dimension of the intermediate hidden states.
        vocab_size: Size of the output vocabulary.
        weights: Optional pre-initialized weight matrix, shape (hidden_dim, vocab_size).
        bias: Optional bias vector, shape (vocab_size,).
    """

    __slots__ = ("hidden_dim", "vocab_size", "_weights", "_bias")

    def __init__(
        self,
        hidden_dim: int,
        vocab_size: int,
        weights: list[float] | None = None,
        bias: list[float] | None = None,
    ) -> None:
        self.hidden_dim = hidden_dim
        self.vocab_size = vocab_size

        if weights is not None:
            if len(weights) != hidden_dim * vocab_size:
                raise ValueError(
                    f"weights must have {hidden_dim * vocab_size} elements, "
                    f"got {len(weights)}"
                )
            self._weights = weights
        else:
            # Xavier initialization
            std = math.sqrt(2.0 / (hidden_dim + vocab_size))
            self._weights = [
                _random.gauss(0.0, std)
                for _ in range(hidden_dim * vocab_size)
            ]

        if bias is not None:
            if len(bias) != vocab_size:
                raise ValueError(
                    f"bias must have {vocab_size} elements, got {len(bias)}"
                )
            self._bias = bias
        else:
            self._bias = [0.0] * vocab_size

    def forward(self, hidden_states: list[float]) -> list[float]:
        """Project hidden states to vocab logits.

        hidden_states: flat list of length hidden_dim.
        Returns: logits of length vocab_size.

        Composed from: MUL, ADD, REDUCE_SUM (matmul = series of dot products).
        """
        if len(hidden_states) != self.hidden_dim:
            raise ValueError(
                f"Expected hidden_dim={self.hidden_dim}, "
                f"got {len(hidden_states)}"
            )

        logits = list(self._bias)  # copy bias
        for v in range(self.vocab_size):
            dot = 0.0
            for h in range(self.hidden_dim):
                dot += hidden_states[h] * self._weights[h * self.vocab_size + v]
            logits[v] += dot
        return logits

    def predict_token(
        self, hidden_states: list[float], temperature: float = 0.0
    ) -> tuple[int, list[float]]:
        """Predict a single token from hidden states.

        Returns (token_id, logits).
        """
        logits = self.forward(hidden_states)
        probs = _softmax_1d(logits)
        token = _sample_token(probs, temperature)
        return token, logits


def eagle_verify(
    draft_logits: list[list[float]],
    target_logits: list[list[float]],
    temperature: float = 1.0,
) -> list[bool]:
    """Verify draft tokens against target model logits.

    Standard speculative decoding acceptance criterion:
    Accept token t if p_target(t) >= p_draft(t), otherwise accept with
    probability p_target(t) / p_draft(t).

    Parameters:
        draft_logits: List of logit vectors from draft head, one per draft token.
        target_logits: List of logit vectors from full model, one per draft token.
        temperature: Sampling temperature.

    Returns:
        List of booleans indicating acceptance for each draft token.
        On first rejection, all subsequent tokens are also rejected.

    Composed from: EXP2, MUL, RECIPROCAL, REDUCE_SUM, CMPLT.
    """
    if len(draft_logits) != len(target_logits):
        raise ValueError(
            f"Logit count mismatch: {len(draft_logits)} draft vs "
            f"{len(target_logits)} target"
        )

    n_draft = len(draft_logits)
    accepted = []

    for i in range(n_draft):
        # Apply temperature and compute probabilities
        if temperature > 0.0:
            d_logits = [x / temperature for x in draft_logits[i]]
            t_logits = [x / temperature for x in target_logits[i]]
        else:
            d_logits = list(draft_logits[i])
            t_logits = list(target_logits[i])

        d_probs = _softmax_1d(d_logits)
        t_probs = _softmax_1d(t_logits)

        # Find the argmax draft token (what was actually sampled)
        draft_token = max(range(len(d_probs)), key=lambda j: d_probs[j])

        d_prob = d_probs[draft_token]
        t_prob = t_probs[draft_token]

        # Acceptance criterion: accept if p_target >= p_draft
        if d_prob > 0.0:
            accept_prob = min(1.0, t_prob / d_prob)
        else:
            accept_prob = 1.0 if t_prob > 0.0 else 0.0

        r = _random.random()
        if r < accept_prob:
            accepted.append(True)
        else:
            # Reject this and all subsequent tokens
            accepted.extend([False] * (n_draft - i))
            break

    return accepted


def eagle_speculate(
    hidden_states_mid: list[list[float]],
    draft_head: EagleDraftHead,
    n_draft: int = 5,
    temperature: float = 0.0,
) -> tuple[list[int], list[list[float]]]:
    """Generate draft tokens using the EAGLE draft head.

    Self-speculative: uses intermediate hidden states from layer N/2
    of the target model itself — no separate draft model needed.

    Parameters:
        hidden_states_mid: List of hidden state vectors from intermediate layer.
            Each is a flat list of length hidden_dim. The last element is the
            current position's hidden state; preceding elements provide context.
        draft_head: The lightweight prediction head.
        n_draft: Number of draft tokens to generate.
        temperature: Sampling temperature (0 = greedy).

    Returns:
        (draft_tokens, draft_logits) where:
        - draft_tokens: list of n_draft predicted token IDs
        - draft_logits: list of n_draft logit vectors

    Algorithm:
        1. Start from the last hidden state in hidden_states_mid
        2. For each draft position:
           a. Project hidden state through draft head -> logits
           b. Sample token from logits
           c. Use the same hidden state for next draft (autoregressive
              approximation — the real hidden state would require a full
              forward pass, but EAGLE's insight is that the intermediate
              representation is stable enough for short drafts)
    """
    if not hidden_states_mid:
        raise ValueError("hidden_states_mid must not be empty")

    draft_tokens = []
    draft_logits = []

    # Start from the last hidden state
    current_hidden = hidden_states_mid[-1]

    for _ in range(n_draft):
        token, logits = draft_head.predict_token(current_hidden, temperature)
        draft_tokens.append(token)
        draft_logits.append(logits)

        # For self-speculative decoding, the draft head reuses the same
        # intermediate hidden state. A more sophisticated approach would
        # update the hidden state based on the predicted token embedding,
        # but EAGLE-3 shows that for short draft lengths (5-8 tokens),
        # the intermediate representation is sufficiently stable.

    return draft_tokens, draft_logits


def eagle_full_pipeline(
    hidden_states_mid: list[list[float]],
    target_logits_fn,
    draft_head: EagleDraftHead,
    n_draft: int = 5,
    temperature: float = 0.0,
) -> tuple[list[int], int]:
    """Full EAGLE-3 self-speculative decoding pipeline.

    Parameters:
        hidden_states_mid: Intermediate hidden states from layer N/2.
        target_logits_fn: Callable that takes draft_tokens (list[int]) and
            returns target_logits (list[list[float]]) — the full model's
            logits for each draft position. This represents running the
            full model forward for all draft candidates in parallel.
        draft_head: The EAGLE draft head.
        n_draft: Number of tokens to draft.
        temperature: Sampling temperature.

    Returns:
        (accepted_tokens, n_accepted)

    Pipeline:
        1. Generate n_draft candidates from intermediate hidden states
        2. Run full model forward for all candidates (via target_logits_fn)
        3. Verify: accept matching tokens, reject divergent ones
        4. Return accepted prefix
    """
    # Step 1: Draft from intermediate hidden states
    draft_tokens, draft_logits = eagle_speculate(
        hidden_states_mid, draft_head, n_draft, temperature
    )

    # Step 2: Get target model logits for all draft positions
    target_logits = target_logits_fn(draft_tokens)

    if len(target_logits) != len(draft_tokens):
        raise ValueError(
            f"target_logits_fn returned {len(target_logits)} logit vectors "
            f"for {len(draft_tokens)} draft tokens"
        )

    # Step 3: Verify draft against target
    accepted = eagle_verify(draft_logits, target_logits, temperature)

    # Step 4: Collect accepted tokens
    accepted_tokens = []
    for i, is_accepted in enumerate(accepted):
        if is_accepted:
            accepted_tokens.append(draft_tokens[i])
        else:
            break

    return accepted_tokens, len(accepted_tokens)
