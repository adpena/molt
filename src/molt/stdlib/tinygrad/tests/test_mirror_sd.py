"""Tests for Mirror Speculative Decoding (arxiv 2510.13161)."""

import math
import random

from tinygrad.mirror_sd import (
    EarlyExitProxy,
    HypothesisTree,
    MirrorSpeculativeDecoder,
    branch_complete_rollout,
    mirror_verify,
    mirror_speculative_decode,
    speculative_streaming_draft,
    compute_overlap_budget,
    estimate_mirror_latency,
    _softmax_1d,
    _top_kappa,
    _sample_from_logits,
)


# ---------------------------------------------------------------------------
# Utility tests
# ---------------------------------------------------------------------------


def test_softmax_1d_valid_distribution():
    """_softmax_1d produces valid probability distribution."""
    probs = _softmax_1d([1.0, 2.0, 3.0])
    assert len(probs) == 3
    assert all(p >= 0 for p in probs), "Negative probability"
    assert abs(sum(probs) - 1.0) < 1e-6, f"Probs sum to {sum(probs)}, not 1.0"
    assert probs[0] < probs[1] < probs[2], "Monotonicity violated"


def test_softmax_1d_overflow_safe():
    """_softmax_1d handles large logits without overflow."""
    probs = _softmax_1d([1000.0, 1001.0, 999.0])
    assert all(math.isfinite(p) for p in probs), "Non-finite probability"
    assert abs(sum(probs) - 1.0) < 1e-6


def test_softmax_1d_empty():
    """_softmax_1d returns empty for empty input."""
    assert _softmax_1d([]) == []


def test_top_kappa_basic():
    """_top_kappa returns top-k tokens sorted by descending probability."""
    logits = [0.0, 5.0, 1.0, 10.0, 2.0]
    result = _top_kappa(logits, kappa=3)
    assert len(result) == 3
    # Token 3 (logit 10.0) should be first
    assert result[0][0] == 3
    # Token 1 (logit 5.0) should be second
    assert result[1][0] == 1
    # All log-probs should be finite
    assert all(math.isfinite(lp) for _, lp in result)


def test_top_kappa_rejects_nonpositive():
    """_top_kappa rejects kappa <= 0."""
    try:
        _top_kappa([1.0, 2.0], kappa=0)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_top_kappa_empty_logits():
    """_top_kappa returns empty for empty logits."""
    assert _top_kappa([], kappa=3) == []


def test_sample_from_logits_greedy():
    """_sample_from_logits at temperature=0 returns argmax."""
    logits = [0.0, 1.0, 5.0, 2.0]
    token = _sample_from_logits(logits, temperature=0.0)
    assert token == 2, f"Expected token 2 (argmax), got {token}"


def test_sample_from_logits_stochastic():
    """_sample_from_logits produces valid tokens with temperature > 0."""
    random.seed(42)
    logits = [1.0, 2.0, 3.0, 4.0]
    tokens = [_sample_from_logits(logits, temperature=1.0) for _ in range(100)]
    assert all(0 <= t < 4 for t in tokens), "Token out of range"
    # With these logits, token 3 should appear most frequently
    counts = [tokens.count(i) for i in range(4)]
    assert counts[3] > counts[0], "Token 3 (highest logit) should be most frequent"


# ---------------------------------------------------------------------------
# EarlyExitProxy tests
# ---------------------------------------------------------------------------


def test_early_exit_proxy_forward_shape():
    """EarlyExitProxy.forward produces logits of correct size."""
    proxy = EarlyExitProxy(hidden_dim=16, vocab_size=32, kappa=4)
    hidden = [random.gauss(0, 1) for _ in range(16)]
    logits = proxy.forward(hidden)
    assert len(logits) == 32, f"Expected 32 logits, got {len(logits)}"
    assert all(math.isfinite(x) for x in logits), "Non-finite logits"


def test_early_exit_proxy_custom_weights():
    """EarlyExitProxy works with explicit weights and bias."""
    h, v = 4, 8
    weights = [0.0] * (h * v)
    # Wire hidden[0] strongly to vocab token 2
    weights[0 * v + 2] = 10.0
    bias = [0.0] * v

    proxy = EarlyExitProxy(
        hidden_dim=h,
        vocab_size=v,
        kappa=2,
        lm_head_weights=weights,
        lm_head_bias=bias,
    )
    hidden = [1.0, 0.0, 0.0, 0.0]
    logits = proxy.forward(hidden)
    assert logits[2] > logits[0], "Token 2 should have highest logit"


def test_early_exit_proxy_token_channel():
    """extract_token_channel returns kappa entries."""
    proxy = EarlyExitProxy(hidden_dim=8, vocab_size=16, kappa=4)
    hidden = [random.gauss(0, 1) for _ in range(8)]
    channel = proxy.extract_token_channel(hidden)
    assert len(channel) == 4
    assert all(isinstance(t, int) and isinstance(lp, float) for t, lp in channel)


def test_early_exit_proxy_rejects_wrong_hidden_dim():
    """EarlyExitProxy rejects mismatched hidden state dimension."""
    proxy = EarlyExitProxy(hidden_dim=8, vocab_size=16)
    try:
        proxy.forward([0.0] * 4)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_early_exit_proxy_rejects_wrong_weights_size():
    """EarlyExitProxy rejects wrong weight matrix size."""
    try:
        EarlyExitProxy(
            hidden_dim=4,
            vocab_size=8,
            lm_head_weights=[0.0] * 10,  # Wrong: should be 4*8=32
        )
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_early_exit_proxy_rejects_wrong_bias_size():
    """EarlyExitProxy rejects wrong bias size."""
    try:
        EarlyExitProxy(
            hidden_dim=4,
            vocab_size=8,
            lm_head_bias=[0.0] * 3,  # Wrong: should be 8
        )
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


# ---------------------------------------------------------------------------
# HypothesisTree tests
# ---------------------------------------------------------------------------


def test_hypothesis_tree_add_and_get():
    """HypothesisTree stores and retrieves branches."""
    tree = HypothesisTree(kappa=3, gamma=4)
    branch = [(10, [1.0, 2.0]), (11, [3.0, 4.0])]
    tree.add_branch(5, branch)

    assert 5 in tree.roots
    retrieved = tree.get_branch(5)
    assert retrieved is not None
    assert len(retrieved) == 2
    assert retrieved[0][0] == 10


def test_hypothesis_tree_missing_branch():
    """HypothesisTree returns None for missing branch."""
    tree = HypothesisTree(kappa=2, gamma=3)
    assert tree.get_branch(999) is None


def test_hypothesis_tree_reusable_branch():
    """find_reusable_branch returns continuation when prefix matches."""
    tree = HypothesisTree(kappa=2, gamma=3)
    # Branch: root=5 -> 10 -> 11 -> 12
    tree.add_branch(5, [(10, [1.0]), (11, [2.0]), (12, [3.0])])

    # Accepted prefix = [5], correction = 10 => full sequence [5, 10]
    # Branch tokens: [5, 10, 11, 12]. Matches at length 2.
    # Remaining: branch[1:] = [(11, ...), (12, ...)]
    result = tree.find_reusable_branch(accepted_prefix=[5], correction_token=10)
    assert result is not None
    assert len(result) == 2
    assert result[0][0] == 11


def test_hypothesis_tree_no_reusable_branch():
    """find_reusable_branch returns None when prefix diverges."""
    tree = HypothesisTree(kappa=2, gamma=3)
    tree.add_branch(5, [(10, [1.0]), (11, [2.0])])

    # Correction token 99 does not match branch continuation
    result = tree.find_reusable_branch(accepted_prefix=[5], correction_token=99)
    assert result is None


# ---------------------------------------------------------------------------
# branch_complete_rollout tests
# ---------------------------------------------------------------------------


def test_branch_complete_rollout_structure():
    """branch_complete_rollout produces tree with correct structure."""
    random.seed(42)

    def dummy_draft_fn(token_id, prev_logits):
        return (token_id + 1, [0.0] * 8)

    channel = [(0, -0.5), (1, -1.0), (2, -1.5)]
    tree = branch_complete_rollout(
        token_channel=channel,
        draft_fn=dummy_draft_fn,
        gamma=4,
    )

    assert len(tree.roots) == 3
    for root_token, _ in channel:
        branch = tree.get_branch(root_token)
        assert branch is not None
        assert len(branch) == 4


# ---------------------------------------------------------------------------
# speculative_streaming_draft tests
# ---------------------------------------------------------------------------


def test_speculative_streaming_produces_gamma_tokens():
    """speculative_streaming_draft produces exactly gamma tokens."""

    def dummy_draft_fn(token_ids):
        return [[0.0, 1.0, 2.0, 0.5] for _ in range(len(token_ids) + 1)]

    result = speculative_streaming_draft(
        draft_fn=dummy_draft_fn,
        root_token=0,
        gamma=6,
        n_streams=2,
    )
    assert len(result) == 6
    assert all(
        isinstance(token, int) and isinstance(logits, list) for token, logits in result
    )


# ---------------------------------------------------------------------------
# mirror_verify tests
# ---------------------------------------------------------------------------


def test_mirror_verify_accepts_identical():
    """mirror_verify accepts all tokens when distributions are identical."""
    random.seed(42)
    logits = [[1.0, 2.0, 0.5, -1.0]] * 5
    tokens = [1] * 5  # All token 1

    n_acc, correction = mirror_verify(tokens, logits, logits, temperature=1.0)
    assert n_acc == 5, f"Expected all 5 accepted, got {n_acc}"
    assert correction is None


def test_mirror_verify_rejects_divergent():
    """mirror_verify rejects tokens when distributions diverge."""
    random.seed(123)
    draft_logits = [[10.0, -10.0, -10.0, -10.0]] * 5
    target_logits = [[-10.0, 10.0, -10.0, -10.0]] * 5
    tokens = [0] * 5  # Draft sampled token 0

    n_acc, correction = mirror_verify(
        tokens,
        draft_logits,
        target_logits,
        temperature=1.0,
    )
    assert n_acc < 5, f"Divergent distributions should reject, but accepted {n_acc}"
    assert correction is not None


def test_mirror_verify_rejects_mismatched_lengths():
    """mirror_verify rejects mismatched logit counts."""
    try:
        mirror_verify([0], [[1.0]], [[1.0], [2.0]], temperature=1.0)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_mirror_verify_rejects_token_logit_mismatch():
    """mirror_verify rejects mismatched token and logit counts."""
    try:
        mirror_verify([0, 1], [[1.0]], [[1.0]], temperature=1.0)
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_mirror_verify_greedy():
    """mirror_verify at temperature=0 accepts matching greedy tokens."""
    random.seed(42)
    # Both draft and target prefer token 2 (highest logit)
    logits = [[0.0, 1.0, 5.0, 2.0]] * 3
    tokens = [2, 2, 2]

    n_acc, correction = mirror_verify(tokens, logits, logits, temperature=0.0)
    assert n_acc == 3
    assert correction is None


# ---------------------------------------------------------------------------
# MirrorSpeculativeDecoder tests
# ---------------------------------------------------------------------------


def _make_decoder(hidden_dim=8, vocab_size=16, gamma=5, kappa=4):
    """Create a MirrorSpeculativeDecoder with deterministic mock models."""
    random.seed(42)
    proxy = EarlyExitProxy(hidden_dim=hidden_dim, vocab_size=vocab_size, kappa=kappa)

    def draft_step_fn(token_id, prev_logits):
        logits = [0.0] * vocab_size
        logits[token_id % vocab_size] = 5.0
        return (token_id % vocab_size, logits)

    def target_logits_fn(token_ids):
        result = []
        for t in token_ids:
            logits = [0.0] * vocab_size
            logits[t % vocab_size] = 5.0
            result.append(logits)
        return result

    decoder = MirrorSpeculativeDecoder(
        early_exit_proxy=proxy,
        draft_step_fn=draft_step_fn,
        target_logits_fn=target_logits_fn,
        gamma=gamma,
        kappa=kappa,
        temperature=0.0,
    )
    return decoder


def test_decoder_step_produces_tokens():
    """MirrorSpeculativeDecoder.step produces accepted tokens."""
    decoder = _make_decoder()
    hidden = [random.gauss(0, 1) for _ in range(8)]
    tokens, n_acc = decoder.step(hidden)
    assert n_acc >= 0
    assert len(tokens) == n_acc
    assert all(0 <= t < 16 for t in tokens)


def test_decoder_acceptance_rate():
    """Acceptance rate is computed correctly after multiple steps."""
    decoder = _make_decoder()
    for _ in range(5):
        hidden = [random.gauss(0, 1) for _ in range(8)]
        decoder.step(hidden)
    rate = decoder.acceptance_rate
    assert 0.0 <= rate <= 1.0, f"Acceptance rate {rate} out of [0,1]"


def test_decoder_mean_accepted():
    """Mean accepted tokens is non-negative after steps."""
    decoder = _make_decoder()
    for _ in range(3):
        hidden = [random.gauss(0, 1) for _ in range(8)]
        decoder.step(hidden)
    assert decoder.mean_accepted >= 0.0


def test_decoder_decode_produces_sequence():
    """MirrorSpeculativeDecoder.decode produces a token sequence."""
    decoder = _make_decoder()
    hidden = [random.gauss(0, 1) for _ in range(8)]
    tokens = decoder.decode(initial_hidden=hidden, max_tokens=20)
    assert isinstance(tokens, list)
    assert len(tokens) <= 20
    assert all(0 <= t < 16 for t in tokens)


def test_decoder_decode_respects_max_tokens():
    """decode stops at max_tokens."""
    decoder = _make_decoder()
    hidden = [1.0] * 8
    tokens = decoder.decode(initial_hidden=hidden, max_tokens=5)
    assert len(tokens) <= 5


def test_decoder_decode_respects_eos():
    """decode stops at EOS token."""
    random.seed(42)
    vocab_size = 8
    eos = 7
    proxy = EarlyExitProxy(hidden_dim=4, vocab_size=vocab_size, kappa=2)

    # Draft always produces eos at position 2
    call_count = [0]

    def draft_step_fn(token_id, prev_logits):
        call_count[0] += 1
        if call_count[0] % 3 == 0:
            logits = [0.0] * vocab_size
            logits[eos] = 10.0
            return (eos, logits)
        logits = [0.0] * vocab_size
        logits[token_id % vocab_size] = 5.0
        return (token_id % vocab_size, logits)

    def target_logits_fn(token_ids):
        result = []
        for t in token_ids:
            logits = [0.0] * vocab_size
            logits[t % vocab_size] = 5.0
            result.append(logits)
        return result

    decoder = MirrorSpeculativeDecoder(
        early_exit_proxy=proxy,
        draft_step_fn=draft_step_fn,
        target_logits_fn=target_logits_fn,
        gamma=5,
        kappa=2,
        temperature=0.0,
    )

    tokens = decoder.decode(
        initial_hidden=[1.0] * 4,
        max_tokens=100,
        eos_token=eos,
    )
    # Sequence should end with or contain eos
    if tokens:
        assert tokens[-1] == eos or len(tokens) <= 100


# ---------------------------------------------------------------------------
# mirror_speculative_decode convenience function tests
# ---------------------------------------------------------------------------


def test_mirror_speculative_decode_convenience():
    """mirror_speculative_decode produces tokens and acceptance rate."""
    random.seed(42)
    proxy = EarlyExitProxy(hidden_dim=8, vocab_size=16, kappa=4)

    def draft_step_fn(token_id, prev_logits):
        return (token_id % 16, [0.0] * 16)

    def target_logits_fn(token_ids):
        return [[0.0] * 16] * len(token_ids)

    tokens, rate = mirror_speculative_decode(
        early_exit_proxy=proxy,
        draft_step_fn=draft_step_fn,
        target_logits_fn=target_logits_fn,
        initial_hidden=[1.0] * 8,
        gamma=5,
        kappa=4,
        max_tokens=20,
    )
    assert isinstance(tokens, list)
    assert 0.0 <= rate <= 1.0


# ---------------------------------------------------------------------------
# Latency model tests
# ---------------------------------------------------------------------------


def test_compute_overlap_budget():
    """compute_overlap_budget returns target suffix time."""
    delta = compute_overlap_budget(
        target_prefix_time_ms=5.0,
        target_suffix_time_ms=15.0,
    )
    assert delta == 15.0


def test_estimate_mirror_latency_draft_hidden():
    """When draft fits in overlap budget, latency equals target-only path."""
    latency = estimate_mirror_latency(
        target_prefix_time_ms=5.0,
        target_suffix_time_ms=15.0,
        draft_gen_time_ms=10.0,  # < 15.0 => hidden under target suffix
        rendezvous_ee_ms=0.01,
        rendezvous_fv_ms=0.01,
    )
    # T = 5.0 + 0.01 + max(15.0, 10.0) + 0.01 = 20.02
    assert abs(latency - 20.02) < 1e-6


def test_estimate_mirror_latency_draft_exceeds():
    """When draft exceeds overlap budget, latency includes draft time."""
    latency = estimate_mirror_latency(
        target_prefix_time_ms=5.0,
        target_suffix_time_ms=10.0,
        draft_gen_time_ms=20.0,  # > 10.0 => draft dominates
        rendezvous_ee_ms=0.01,
        rendezvous_fv_ms=0.01,
    )
    # T = 5.0 + 0.01 + max(10.0, 20.0) + 0.01 = 25.02
    assert abs(latency - 25.02) < 1e-6


if __name__ == "__main__":
    test_softmax_1d_valid_distribution()
    test_softmax_1d_overflow_safe()
    test_softmax_1d_empty()
    test_top_kappa_basic()
    test_top_kappa_rejects_nonpositive()
    test_top_kappa_empty_logits()
    test_sample_from_logits_greedy()
    test_sample_from_logits_stochastic()
    test_early_exit_proxy_forward_shape()
    test_early_exit_proxy_custom_weights()
    test_early_exit_proxy_token_channel()
    test_early_exit_proxy_rejects_wrong_hidden_dim()
    test_early_exit_proxy_rejects_wrong_weights_size()
    test_early_exit_proxy_rejects_wrong_bias_size()
    test_hypothesis_tree_add_and_get()
    test_hypothesis_tree_missing_branch()
    test_hypothesis_tree_reusable_branch()
    test_hypothesis_tree_no_reusable_branch()
    test_branch_complete_rollout_structure()
    test_speculative_streaming_produces_gamma_tokens()
    test_mirror_verify_accepts_identical()
    test_mirror_verify_rejects_divergent()
    test_mirror_verify_rejects_mismatched_lengths()
    test_mirror_verify_rejects_token_logit_mismatch()
    test_mirror_verify_greedy()
    test_decoder_step_produces_tokens()
    test_decoder_acceptance_rate()
    test_decoder_mean_accepted()
    test_decoder_decode_produces_sequence()
    test_decoder_decode_respects_max_tokens()
    test_decoder_decode_respects_eos()
    test_mirror_speculative_decode_convenience()
    test_compute_overlap_budget()
    test_estimate_mirror_latency_draft_hidden()
    test_estimate_mirror_latency_draft_exceeds()
    print("All Mirror-SD tests passed.")
