"""Tests for EAGLE-3 self-speculative decoding."""

from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")


import math
import random

from tinygrad.eagle import (
    EagleDraftHead,
    eagle_verify,
    eagle_speculate,
    eagle_full_pipeline,
    _softmax_1d,
)


def test_draft_head_produces_valid_logits():
    """EagleDraftHead forward produces logits of correct size."""
    head = EagleDraftHead(hidden_dim=16, vocab_size=32, bias=None)
    hidden = [random.gauss(0, 1) for _ in range(16)]
    logits = head.forward(hidden)

    assert len(logits) == 32, f"Expected 32 logits, got {len(logits)}"
    # Logits should be finite
    assert all(math.isfinite(x) for x in logits), "Non-finite logits"


def test_draft_head_valid_distribution():
    """EagleDraftHead produces valid probability distribution after softmax."""
    head = EagleDraftHead(hidden_dim=8, vocab_size=16)
    hidden = [random.gauss(0, 1) for _ in range(8)]
    logits = head.forward(hidden)
    probs = _softmax_1d(logits)

    assert len(probs) == 16
    assert all(p >= 0 for p in probs), "Negative probability"
    assert abs(sum(probs) - 1.0) < 1e-6, f"Probs sum to {sum(probs)}, not 1.0"


def test_draft_head_predict_token():
    """predict_token returns valid token and logits."""
    head = EagleDraftHead(hidden_dim=8, vocab_size=10)
    hidden = [0.5] * 8
    token, logits = head.predict_token(hidden, temperature=0.0)

    assert 0 <= token < 10, f"Token {token} out of range"
    assert len(logits) == 10


def test_draft_head_custom_weights():
    """EagleDraftHead works with custom weights."""
    # Create a simple head where weights push toward token 0
    weights = [0.0] * (4 * 8)
    # Set weights so that hidden=[1,0,0,0] -> logits strongly favor token 0
    weights[0 * 8 + 0] = 10.0  # hidden[0] * w[0,0] = 10
    bias = [0.0] * 8

    head = EagleDraftHead(hidden_dim=4, vocab_size=8, weights=weights, bias=bias)
    hidden = [1.0, 0.0, 0.0, 0.0]
    token, logits = head.predict_token(hidden, temperature=0.0)

    assert token == 0, f"Expected token 0, got {token} (logits: {logits})"
    assert logits[0] > logits[1], "Token 0 should have highest logit"


def test_draft_head_rejects_wrong_hidden_dim():
    """EagleDraftHead rejects wrong hidden_dim."""
    head = EagleDraftHead(hidden_dim=8, vocab_size=16)
    try:
        head.forward([0.0] * 4)  # Wrong size
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_verify_accepts_identical_distributions():
    """eagle_verify accepts tokens when draft and target distributions are identical."""
    random.seed(42)
    logits = [[1.0, 2.0, 0.5, -1.0]] * 5

    # With identical distributions, acceptance prob = 1.0 for all
    accepted = eagle_verify(logits, logits, temperature=1.0)

    assert len(accepted) == 5
    assert all(accepted), "Identical distributions should always be accepted"


def test_verify_rejects_divergent_distributions():
    """eagle_verify rejects tokens when distributions diverge significantly."""
    random.seed(123)

    # Draft strongly prefers token 0
    draft_logits = [[10.0, -10.0, -10.0, -10.0]] * 5
    # Target strongly prefers token 1
    target_logits = [[-10.0, 10.0, -10.0, -10.0]] * 5

    accepted = eagle_verify(draft_logits, target_logits, temperature=1.0)

    # With strongly divergent distributions, most tokens should be rejected.
    # The first token has near-zero acceptance probability.
    n_accepted = sum(1 for a in accepted if a)
    assert n_accepted < 5, (
        f"Divergent distributions should reject most tokens, "
        f"but accepted {n_accepted}/5"
    )


def test_verify_stops_at_first_rejection():
    """eagle_verify stops accepting after first rejection."""
    random.seed(456)

    # Make second token divergent
    draft_logits = [
        [5.0, -5.0],  # Strong preference for token 0
        [-5.0, 5.0],  # Strong preference for token 1
        [5.0, -5.0],  # Strong preference for token 0
    ]
    target_logits = [
        [5.0, -5.0],  # Same as draft
        [5.0, -5.0],  # Opposite of draft
        [5.0, -5.0],  # Same as draft
    ]

    accepted = eagle_verify(draft_logits, target_logits, temperature=1.0)

    # After the rejected token, all subsequent must be False
    found_rejection = False
    for a in accepted:
        if found_rejection:
            assert not a, "No tokens should be accepted after first rejection"
        if not a:
            found_rejection = True


def test_speculate_produces_valid_tokens():
    """eagle_speculate produces valid token IDs and logit vectors."""
    head = EagleDraftHead(hidden_dim=8, vocab_size=16)
    hidden_states = [[random.gauss(0, 1) for _ in range(8)] for _ in range(3)]

    tokens, logits = eagle_speculate(hidden_states, head, n_draft=5)

    assert len(tokens) == 5, f"Expected 5 draft tokens, got {len(tokens)}"
    assert len(logits) == 5, f"Expected 5 logit vectors, got {len(logits)}"
    assert all(0 <= t < 16 for t in tokens), "Token out of vocabulary range"
    assert all(len(logit_vec) == 16 for logit_vec in logits), "Wrong logit vector size"


def test_speculate_empty_hidden_rejects():
    """eagle_speculate rejects empty hidden states."""
    head = EagleDraftHead(hidden_dim=8, vocab_size=16)
    try:
        eagle_speculate([], head, n_draft=3)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_full_pipeline_produces_valid_output():
    """eagle_full_pipeline produces valid accepted tokens."""
    random.seed(789)
    head = EagleDraftHead(hidden_dim=8, vocab_size=16)
    hidden_states = [[random.gauss(0, 1) for _ in range(8)] for _ in range(3)]

    # Target model that matches draft perfectly
    def target_logits_fn(draft_tokens):
        # Return the same logits the draft head would produce
        # (perfect draft = 100% acceptance)
        hidden = hidden_states[-1]
        logits = head.forward(hidden)
        return [logits] * len(draft_tokens)

    accepted_tokens, n_accepted = eagle_full_pipeline(
        hidden_states, target_logits_fn, head, n_draft=5
    )

    assert n_accepted >= 0
    assert len(accepted_tokens) == n_accepted
    assert all(0 <= t < 16 for t in accepted_tokens)


def test_full_pipeline_with_divergent_target():
    """eagle_full_pipeline handles divergent target model."""
    random.seed(101)
    head = EagleDraftHead(hidden_dim=4, vocab_size=8)
    hidden_states = [[1.0, 0.0, 0.0, 0.0]]

    # Target model that always returns uniform distribution
    def target_logits_fn(draft_tokens):
        return [[0.0] * 8] * len(draft_tokens)

    accepted_tokens, n_accepted = eagle_full_pipeline(
        hidden_states, target_logits_fn, head, n_draft=5
    )

    # Should produce some result (possibly 0 accepted)
    assert n_accepted >= 0
    assert len(accepted_tokens) == n_accepted


def test_softmax_1d_valid():
    """_softmax_1d produces valid probability distribution."""
    probs = _softmax_1d([1.0, 2.0, 3.0])
    assert len(probs) == 3
    assert all(p >= 0 for p in probs)
    assert abs(sum(probs) - 1.0) < 1e-6
    # Monotonically increasing (larger logit = larger prob)
    assert probs[0] < probs[1] < probs[2]


def test_softmax_1d_overflow_safe():
    """_softmax_1d handles large logits without overflow."""
    probs = _softmax_1d([1000.0, 1001.0, 999.0])
    assert all(math.isfinite(p) for p in probs)
    assert abs(sum(probs) - 1.0) < 1e-6


if __name__ == "__main__":
    test_draft_head_produces_valid_logits()
    test_draft_head_valid_distribution()
    test_draft_head_predict_token()
    test_draft_head_custom_weights()
    test_draft_head_rejects_wrong_hidden_dim()
    test_verify_accepts_identical_distributions()
    test_verify_rejects_divergent_distributions()
    test_verify_stops_at_first_rejection()
    test_speculate_produces_valid_tokens()
    test_speculate_empty_hidden_rejects()
    test_full_pipeline_produces_valid_output()
    test_full_pipeline_with_divergent_target()
    test_softmax_1d_valid()
    test_softmax_1d_overflow_safe()
    print("All EAGLE-3 tests passed.")
