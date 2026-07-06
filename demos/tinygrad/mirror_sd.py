"""
tinygrad.mirror_sd -- Mirror Speculative Decoding.

Implements the Mirror-SD algorithm (Bhendawade et al., arxiv 2510.13161):
parallel draft-target execution with branch-complete rollouts from early-exit
signals, bidirectional speculation, and multi-token speculative streaming.

Architecture:
- EarlyExitProxy: extracts top-kappa token channel from intermediate layer l_e
- HypothesisTree: branch-complete rollout structure for precomputed continuations
- MirrorSpeculativeDecoder: full pipeline orchestrating parallel devices
- SpeculativeStream: multi-token draft emission per forward pass

All operations are composed from the 26 tinygrad primitives:
MUL, ADD, EXP2, LOG2, RECIPROCAL, REDUCE_SUM, REDUCE_MAX, CMPLT, WHERE, MAX.

Reference: "Mirror Speculative Decoding: Breaking the Serial Barrier in LLM
Inference", Bhendawade et al., Apple, 2025. arXiv:2510.13161.
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")


import math
import random as _random
from typing import Callable


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


def _top_kappa(logits: list[float], kappa: int) -> list[tuple[int, float]]:
    """Extract top-kappa tokens with their log-probabilities.

    Returns list of (token_id, log_prob) pairs sorted by descending probability.
    This is the token channel M_t from Eq. 7 of Mirror-SD.

    Composed from: REDUCE_MAX, CMPLT, LOG2.
    """
    if kappa <= 0:
        raise ValueError("kappa must be positive")
    if not logits:
        return []

    probs = _softmax_1d(logits)
    indexed = [(i, probs[i]) for i in range(len(probs))]
    indexed.sort(key=lambda x: x[1], reverse=True)
    top = indexed[:kappa]

    result = []
    for token_id, prob in top:
        log_prob = math.log(prob) if prob > 0.0 else float("-inf")
        result.append((token_id, log_prob))
    return result


def _sample_from_logits(logits: list[float], temperature: float = 1.0) -> int:
    """Sample a token from logits with temperature.

    Composed from: EXP2, MUL, RECIPROCAL, REDUCE_SUM.
    """
    if temperature == 0.0:
        return max(range(len(logits)), key=lambda i: logits[i])

    scaled = [x / temperature for x in logits]
    probs = _softmax_1d(scaled)
    r = _random.random()
    cumsum = 0.0
    for i, p in enumerate(probs):
        cumsum += p
        if r < cumsum:
            return i
    return len(probs) - 1


class EarlyExitProxy:
    """Extracts proxy next-token distribution from intermediate layer l_e.

    At early-exit layer l_e < N, applying the LM head W_LM to intermediate
    hidden state h_t^(l_e) yields a proxy distribution:

        p^(l_e)(. | y_{<t}, x) = Softmax(W_LM * h_t^(l_e))   [Eq. 6]

    which is typically strongly correlated with the final distribution p^(N).

    We expose a low-bandwidth token channel M_t containing only the top-kappa
    candidates and their log-probabilities (Eq. 7).

    Parameters:
        lm_head_weights: flat list of (hidden_dim * vocab_size) floats.
        lm_head_bias: flat list of vocab_size floats, or None.
        hidden_dim: dimension of hidden states at layer l_e.
        vocab_size: vocabulary size.
        kappa: number of top candidates to emit in token channel.
    """

    __slots__ = ("hidden_dim", "vocab_size", "kappa", "_weights", "_bias")

    def __init__(
        self,
        hidden_dim: int,
        vocab_size: int,
        kappa: int = 4,
        lm_head_weights: list[float] | None = None,
        lm_head_bias: list[float] | None = None,
    ) -> None:
        self.hidden_dim = hidden_dim
        self.vocab_size = vocab_size
        self.kappa = kappa

        if lm_head_weights is not None:
            if len(lm_head_weights) != hidden_dim * vocab_size:
                raise ValueError(
                    f"lm_head_weights must have {hidden_dim * vocab_size} "
                    f"elements, got {len(lm_head_weights)}"
                )
            self._weights = lm_head_weights
        else:
            std = math.sqrt(2.0 / (hidden_dim + vocab_size))
            self._weights = [
                _random.gauss(0.0, std) for _ in range(hidden_dim * vocab_size)
            ]

        if lm_head_bias is not None:
            if len(lm_head_bias) != vocab_size:
                raise ValueError(
                    f"lm_head_bias must have {vocab_size} elements, "
                    f"got {len(lm_head_bias)}"
                )
            self._bias = lm_head_bias
        else:
            self._bias = [0.0] * vocab_size

    def forward(self, hidden_state: list[float]) -> list[float]:
        """Project hidden state to vocab logits.

        hidden_state: flat list of length hidden_dim (from layer l_e).
        Returns: logits of length vocab_size.

        Composed from: MUL, ADD (matmul decomposed to dot products).
        """
        if len(hidden_state) != self.hidden_dim:
            raise ValueError(
                f"Expected hidden_dim={self.hidden_dim}, got {len(hidden_state)}"
            )
        logits = list(self._bias)
        for v in range(self.vocab_size):
            dot = 0.0
            for h in range(self.hidden_dim):
                dot += hidden_state[h] * self._weights[h * self.vocab_size + v]
            logits[v] += dot
        return logits

    def extract_token_channel(
        self, hidden_state: list[float]
    ) -> list[tuple[int, float]]:
        """Extract token channel M_t from intermediate hidden state.

        This is Eq. 7: M_t = Top-kappa(p^(l_e)(. | y_{<t}, x)).

        Returns: list of (token_id, log_prob) pairs, length kappa.
        """
        logits = self.forward(hidden_state)
        return _top_kappa(logits, self.kappa)


class HypothesisTree:
    """Branch-complete rollout structure for speculative continuations.

    Given token channel M_t = {(v_i, log_p_i)}_{i=1}^{kappa}, the draft
    expands each candidate v_i into continuations of depth up to gamma,
    producing a hypothesis tree T_t (Eq. 8):

        For all i in {1,...,kappa}, for all r in {1,...,gamma}:
            y'^(i)_{t+1:t+r} ~ f_d(. | y_{<t}, x, y_{t+1} = v_i)

    The tree enables reuse: if the target's correction matches a precomputed
    branch, the next speculation window is immediately available without
    recomputation.

    Structure: dict mapping root token -> list of continuation paths.
    Each path is a list of (token_id, logits) pairs of length up to gamma.
    """

    __slots__ = ("_branches", "kappa", "gamma")

    def __init__(self, kappa: int, gamma: int) -> None:
        self.kappa = kappa
        self.gamma = gamma
        # root_token -> list of continuation paths
        # Each path: [(token_id, logits), ...] of length gamma
        self._branches: dict[int, list[tuple[int, list[float]]]] = {}

    @property
    def roots(self) -> list[int]:
        """Return all root tokens in this tree."""
        return list(self._branches.keys())

    def add_branch(
        self,
        root_token: int,
        continuation: list[tuple[int, list[float]]],
    ) -> None:
        """Add a continuation branch rooted at root_token.

        continuation: list of (token_id, logits) pairs, length up to gamma.
        """
        self._branches[root_token] = continuation

    def get_branch(self, root_token: int) -> list[tuple[int, list[float]]] | None:
        """Get the continuation branch for a root token, or None."""
        return self._branches.get(root_token)

    def find_reusable_branch(
        self,
        accepted_prefix: list[int],
        correction_token: int,
    ) -> list[tuple[int, list[float]]] | None:
        """Check if the corrected prefix matches a precomputed branch.

        This implements the reuse criterion from Section 3.1:
        Pi_t^+ = (Pi_t, c_{t+tau}) in Paths_tau(T_t)

        Returns the remaining continuation after the match point, or None
        if no reusable branch exists.
        """
        # The corrected sequence is accepted_prefix + [correction_token].
        # We need to find a branch in the tree whose path prefix matches
        # this sequence.
        full_sequence = accepted_prefix + [correction_token]

        if not full_sequence:
            return None

        root = full_sequence[0]
        branch = self._branches.get(root)
        if branch is None:
            return None

        # Check if the branch prefix matches the full corrected sequence.
        # The branch tokens are: [root] + [branch[i][0] for i in ...]
        # We need to match full_sequence against this.
        branch_tokens = [root] + [entry[0] for entry in branch]

        # Find how much of full_sequence matches the branch
        match_len = 0
        for i in range(min(len(full_sequence), len(branch_tokens))):
            if full_sequence[i] == branch_tokens[i]:
                match_len = i + 1
            else:
                break

        if match_len < len(full_sequence):
            # The corrected sequence diverges from this branch
            return None

        # Return the remaining continuation after the match point
        remaining_start = match_len - 1  # -1 because branch[0] corresponds to depth 1
        if remaining_start < len(branch):
            return branch[remaining_start:]
        return []


def branch_complete_rollout(
    token_channel: list[tuple[int, float]],
    draft_fn: Callable[[int, list[float] | None], tuple[int, list[float]]],
    gamma: int,
    context_logits: list[float] | None = None,
) -> HypothesisTree:
    """Perform branch-complete rollout from early-exit token channel.

    For each candidate v_i in M_t, generates a continuation of depth gamma
    using the draft model f_d (Eq. 8).

    Parameters:
        token_channel: M_t = [(token_id, log_prob), ...] from early-exit proxy.
        draft_fn: callable(token_id, prev_logits) -> (next_token, logits).
            Represents one step of the draft model's autoregressive generation.
        gamma: speculative window length (depth of each branch).
        context_logits: logits from the current context (for the first draft step).

    Returns:
        HypothesisTree with kappa roots, each having a continuation of depth gamma.
    """
    kappa = len(token_channel)
    tree = HypothesisTree(kappa=kappa, gamma=gamma)

    for root_token, _log_prob in token_channel:
        continuation = []
        prev_logits = context_logits
        current_token = root_token

        for _depth in range(gamma):
            next_token, logits = draft_fn(current_token, prev_logits)
            continuation.append((next_token, logits))
            prev_logits = logits
            current_token = next_token

        tree.add_branch(root_token, continuation)

    return tree


def speculative_streaming_draft(
    draft_fn: Callable[[list[int]], list[list[float]]],
    root_token: int,
    gamma: int,
    n_streams: int = 2,
) -> list[tuple[int, list[float]]]:
    """Multi-token speculative streaming on the draft model.

    Implements SS (Section 3.2): the draft verifies previously proposed tokens
    while generating new speculative tokens in the same forward pass using
    multi-stream attention.

    A single draft internal step emits n_j >= 1 tokens. The number of draft
    steps J required to materialize gamma tokens satisfies:
        J <= ceil(gamma / eta_bar), where eta_bar = avg(n_j)

    Parameters:
        draft_fn: callable(token_ids) -> list of logit vectors.
            Takes a sequence of token IDs and returns logits for each
            position plus lookahead positions.
        root_token: the root token to start drafting from.
        gamma: total number of draft tokens to produce.
        n_streams: number of lookahead streams (tokens emitted per step).

    Returns:
        list of (token_id, logits) pairs, length gamma.
    """
    result: list[tuple[int, list[float]]] = []
    pending_tokens = [root_token]

    while len(result) < gamma:
        # Multi-stream forward: get logits for current tokens + lookahead
        all_logits = draft_fn(pending_tokens)

        if not all_logits:
            break

        # The first len(pending_tokens) logit vectors verify/extend current tokens.
        # Additional vectors (up to n_streams) are lookahead predictions.
        new_tokens = []
        for i, logits in enumerate(all_logits):
            if len(result) >= gamma:
                break

            token = _sample_from_logits(logits, temperature=0.0)
            result.append((token, logits))
            new_tokens.append(token)

        if not new_tokens:
            break

        # Next step: use the last n_streams tokens as input for the next
        # multi-stream step
        pending_tokens = (
            new_tokens[-n_streams:] if len(new_tokens) >= n_streams else new_tokens
        )

    return result[:gamma]


def mirror_verify(
    draft_tokens: list[int],
    draft_logits: list[list[float]],
    target_logits: list[list[float]],
    temperature: float = 1.0,
) -> tuple[int, int | None]:
    """Mirror-SD verification step.

    Standard speculative decoding acceptance criterion applied to the
    parallel-generated draft and target outputs.

    At step t, the target accepts a prefix of length A_t:
        A_t = max{r in {0,...,gamma} : y_hat_{t+j} = y^targ_{t+j} for all j <= r}

    If A_t < gamma, the target emits a correction c_{t+tau} sampled from
    p^(N)(. | y_{<t+tau-1}, x) at index tau = A_t + 1.

    Parameters:
        draft_tokens: list of draft token IDs, length gamma.
        draft_logits: list of logit vectors from draft, one per token.
        target_logits: list of logit vectors from target, one per token.
        temperature: sampling temperature.

    Returns:
        (n_accepted, correction_token) where:
        - n_accepted: number of accepted draft tokens (A_t from the paper)
        - correction_token: the target's correction at position A_t + 1,
          or None if all tokens accepted.

    Composed from: EXP2, MUL, RECIPROCAL, REDUCE_SUM, CMPLT.
    """
    if len(draft_logits) != len(target_logits):
        raise ValueError(
            f"Logit count mismatch: {len(draft_logits)} draft vs "
            f"{len(target_logits)} target"
        )
    if len(draft_tokens) != len(draft_logits):
        raise ValueError(
            f"Token count mismatch: {len(draft_tokens)} tokens vs "
            f"{len(draft_logits)} logits"
        )

    gamma = len(draft_tokens)

    for i in range(gamma):
        # Compute probabilities with temperature
        if temperature > 0.0:
            d_logits_scaled = [x / temperature for x in draft_logits[i]]
            t_logits_scaled = [x / temperature for x in target_logits[i]]
        else:
            d_logits_scaled = list(draft_logits[i])
            t_logits_scaled = list(target_logits[i])

        d_probs = _softmax_1d(d_logits_scaled)
        t_probs = _softmax_1d(t_logits_scaled)

        token = draft_tokens[i]

        # Get probabilities for the draft token
        d_prob = d_probs[token] if token < len(d_probs) else 0.0
        t_prob = t_probs[token] if token < len(t_probs) else 0.0

        # Acceptance probability: min(1, p_target / p_draft)
        if d_prob > 0.0:
            accept_prob = min(1.0, t_prob / d_prob)
        else:
            accept_prob = 1.0 if t_prob > 0.0 else 0.0

        r = _random.random()
        if r >= accept_prob:
            # Rejection at position i. Sample correction from adjusted distribution.
            # Correction distribution: max(0, p_target - p_draft) / Z
            # This is the residual distribution for the correction token.
            vocab_size = len(t_probs)
            residual = [0.0] * vocab_size
            for v in range(vocab_size):
                dp = d_probs[v] if v < len(d_probs) else 0.0
                tp = t_probs[v] if v < len(t_probs) else 0.0
                residual[v] = max(0.0, tp - dp)

            z = sum(residual)
            if z > 0.0:
                inv_z = 1.0 / z
                correction_probs = [r_val * inv_z for r_val in residual]
            else:
                # Fallback: sample from target distribution
                correction_probs = t_probs

            correction_token = _sample_from_logits(
                # Convert probs back to logits for sampling
                [math.log(max(p, 1e-30)) for p in correction_probs],
                temperature=1.0,
            )

            return i, correction_token

    # All tokens accepted
    return gamma, None


class MirrorSpeculativeDecoder:
    """Full Mirror Speculative Decoding pipeline.

    Orchestrates parallel draft-target execution with early-exit proxy,
    branch-complete rollouts, verification with reuse, and speculative streaming.

    The step latency model (Eq. 10):
        T_Mirror = T_target^{1:l_e} + T_rv^{ee}
                   + max(T_target^{l_e+1:N}, T_draft^{gen}(gamma))
                   + T_rv^{fv}

    When T_draft^{gen}(gamma) <= Delta (overlap budget), the draft is free.

    Parameters:
        early_exit_proxy: EarlyExitProxy for extracting token channel M_t.
        draft_step_fn: callable(token_id, prev_logits) -> (next_token, logits).
            One autoregressive step of the draft model.
        target_logits_fn: callable(token_ids) -> list[list[float]].
            Runs the full target model for all positions, returns logits.
        target_suffix_fn: callable(hidden_state_l_e) -> list[float].
            Runs layers l_e+1..N and returns final logits.
        gamma: speculative window length.
        kappa: number of top-kappa candidates in token channel.
        temperature: sampling temperature.
        use_streaming: whether to use speculative streaming on draft.
        n_streams: number of lookahead streams for SS.
    """

    __slots__ = (
        "early_exit_proxy",
        "draft_step_fn",
        "target_logits_fn",
        "target_suffix_fn",
        "gamma",
        "kappa",
        "temperature",
        "use_streaming",
        "n_streams",
        "_prev_tree",
        "_total_accepted",
        "_total_steps",
    )

    def __init__(
        self,
        early_exit_proxy: EarlyExitProxy,
        draft_step_fn: Callable[[int, list[float] | None], tuple[int, list[float]]],
        target_logits_fn: Callable[[list[int]], list[list[float]]],
        target_suffix_fn: Callable[[list[float]], list[float]] | None = None,
        gamma: int = 5,
        kappa: int = 4,
        temperature: float = 0.0,
        use_streaming: bool = False,
        n_streams: int = 2,
    ) -> None:
        self.early_exit_proxy = early_exit_proxy
        self.draft_step_fn = draft_step_fn
        self.target_logits_fn = target_logits_fn
        self.target_suffix_fn = target_suffix_fn
        self.gamma = gamma
        self.kappa = kappa
        self.temperature = temperature
        self.use_streaming = use_streaming
        self.n_streams = n_streams
        self._prev_tree: HypothesisTree | None = None
        self._total_accepted = 0
        self._total_steps = 0

    @property
    def acceptance_rate(self) -> float:
        """Window-normalized acceptance rate rho (Eq. 4).

        rho(gamma; phi, theta) = E[A_t] / gamma
        """
        if self._total_steps == 0:
            return 0.0
        return self._total_accepted / (self._total_steps * self.gamma)

    @property
    def mean_accepted(self) -> float:
        """Mean accepted tokens per step, E[A_t]."""
        if self._total_steps == 0:
            return 0.0
        return self._total_accepted / self._total_steps

    def step(
        self,
        hidden_state_mid: list[float],
        context_logits: list[float] | None = None,
    ) -> tuple[list[int], int]:
        """Execute one Mirror-SD step.

        This implements the full algorithm from Section 3:
        1. Extract token channel M_t from early-exit layer (Eq. 7)
        2. Branch-complete rollout on draft (Eq. 8), parallel with target suffix
        3. Target completes layers l_e+1..N, produces final logits
        4. Verify draft against target (acceptance criterion)
        5. Check reuse criterion for the next window
        6. Return accepted tokens

        Parameters:
            hidden_state_mid: hidden state from intermediate layer l_e.
            context_logits: logits from the current context position.

        Returns:
            (accepted_tokens, n_accepted) where accepted_tokens is the list
            of accepted token IDs and n_accepted is the count (A_t).
        """
        self._total_steps += 1

        # Step 1: Extract token channel M_t from early-exit proxy
        token_channel = self.early_exit_proxy.extract_token_channel(hidden_state_mid)

        if not token_channel:
            return [], 0

        # Step 2: Branch-complete rollout (runs on draft device, parallel with target)
        tree = branch_complete_rollout(
            token_channel=token_channel,
            draft_fn=self.draft_step_fn,
            gamma=self.gamma,
            context_logits=context_logits,
        )

        # Step 3: Collect draft tokens and logits for the most likely root
        # (the root with highest log-probability from M_t)
        best_root_token = token_channel[0][0]  # highest prob candidate
        best_branch = tree.get_branch(best_root_token)

        if best_branch is None:
            return [], 0

        draft_tokens = [best_root_token] + [entry[0] for entry in best_branch]
        # Draft logits: first position uses context logits projected through
        # early-exit proxy, remaining from branch rollout
        first_logits = self.early_exit_proxy.forward(hidden_state_mid)
        draft_logits = [first_logits] + [entry[1] for entry in best_branch]

        # Truncate to gamma tokens
        draft_tokens = draft_tokens[: self.gamma]
        draft_logits = draft_logits[: self.gamma]

        # Step 4: Get target logits (runs on target device, overlapping with draft)
        # In real heterogeneous execution, this ran in parallel with the draft.
        # Here we call it sequentially since we're in pure Python.
        target_logits = self.target_logits_fn(draft_tokens)

        if len(target_logits) < len(draft_tokens):
            # Pad with empty logits if target returned fewer
            draft_tokens = draft_tokens[: len(target_logits)]
            draft_logits = draft_logits[: len(target_logits)]

        # Step 5: Verify draft against target
        n_accepted, correction_token = mirror_verify(
            draft_tokens=draft_tokens,
            draft_logits=draft_logits,
            target_logits=target_logits,
            temperature=self.temperature,
        )

        self._total_accepted += n_accepted
        accepted_tokens = draft_tokens[:n_accepted]

        # Step 6: Check reuse criterion for next window
        if correction_token is not None and n_accepted < len(draft_tokens):
            reusable = tree.find_reusable_branch(
                accepted_prefix=accepted_tokens,
                correction_token=correction_token,
            )
            if reusable is not None:
                # Store tree for potential reuse in next step
                self._prev_tree = tree
            else:
                self._prev_tree = None
        else:
            self._prev_tree = None

        return accepted_tokens, n_accepted

    def decode(
        self,
        initial_hidden: list[float],
        max_tokens: int = 100,
        eos_token: int = -1,
        context_logits: list[float] | None = None,
    ) -> list[int]:
        """Decode a sequence of tokens using Mirror-SD.

        Repeatedly calls step() until max_tokens or EOS.

        Parameters:
            initial_hidden: hidden state from layer l_e for the first position.
            max_tokens: maximum tokens to generate.
            eos_token: token ID that terminates generation (-1 = no EOS).
            context_logits: logits from the context (first position).

        Returns:
            List of generated token IDs.
        """
        all_tokens: list[int] = []
        current_hidden = initial_hidden
        current_logits = context_logits

        while len(all_tokens) < max_tokens:
            accepted, n_accepted = self.step(
                hidden_state_mid=current_hidden,
                context_logits=current_logits,
            )

            if n_accepted == 0:
                break

            all_tokens.extend(accepted)

            # Check for EOS
            if eos_token >= 0 and eos_token in accepted:
                eos_idx = accepted.index(eos_token)
                all_tokens = all_tokens[: len(all_tokens) - len(accepted) + eos_idx + 1]
                break

            # For simplicity in this composition, we reuse the same hidden state.
            # In a real implementation, the hidden state would be updated by
            # running the target model prefix through layers 0..l_e with the
            # new context that includes accepted tokens.

        return all_tokens[:max_tokens]


def mirror_speculative_decode(
    early_exit_proxy: EarlyExitProxy,
    draft_step_fn: Callable[[int, list[float] | None], tuple[int, list[float]]],
    target_logits_fn: Callable[[list[int]], list[list[float]]],
    initial_hidden: list[float],
    gamma: int = 5,
    kappa: int = 4,
    max_tokens: int = 100,
    temperature: float = 0.0,
    eos_token: int = -1,
    context_logits: list[float] | None = None,
) -> tuple[list[int], float]:
    """Convenience function for Mirror Speculative Decoding.

    Creates a MirrorSpeculativeDecoder and runs decode().

    Parameters:
        early_exit_proxy: EarlyExitProxy for token channel extraction.
        draft_step_fn: one autoregressive draft step.
        target_logits_fn: full target model forward.
        initial_hidden: hidden state from layer l_e.
        gamma: speculative window length.
        kappa: top-kappa candidates in token channel.
        max_tokens: maximum tokens to generate.
        temperature: sampling temperature.
        eos_token: EOS token ID (-1 = none).
        context_logits: logits from context.

    Returns:
        (tokens, acceptance_rate) where tokens is the generated sequence
        and acceptance_rate is the mean acceptance rate across all steps.
    """
    decoder = MirrorSpeculativeDecoder(
        early_exit_proxy=early_exit_proxy,
        draft_step_fn=draft_step_fn,
        target_logits_fn=target_logits_fn,
        gamma=gamma,
        kappa=kappa,
        temperature=temperature,
    )

    tokens = decoder.decode(
        initial_hidden=initial_hidden,
        max_tokens=max_tokens,
        eos_token=eos_token,
        context_logits=context_logits,
    )

    return tokens, decoder.acceptance_rate


def compute_overlap_budget(
    target_prefix_time_ms: float,
    target_suffix_time_ms: float,
    rendezvous_time_ms: float = 0.01,
) -> float:
    """Compute the overlap budget Delta for Mirror-SD (Section 3.4).

    Delta = T_target^{l_e+1:N}

    If the draft generation time T_draft^gen(gamma) <= Delta, the entire
    draft is hidden under the target suffix (draft is free).

    Parameters:
        target_prefix_time_ms: time for target layers 0..l_e.
        target_suffix_time_ms: time for target layers l_e+1..N.
        rendezvous_time_ms: token channel transfer overhead.

    Returns:
        Delta in milliseconds.
    """
    return target_suffix_time_ms


def estimate_mirror_latency(
    target_prefix_time_ms: float,
    target_suffix_time_ms: float,
    draft_gen_time_ms: float,
    rendezvous_ee_ms: float = 0.01,
    rendezvous_fv_ms: float = 0.01,
) -> float:
    """Estimate Mirror-SD step latency (Eq. 10).

    T_Mirror = T_target^{1:l_e} + T_rv^{ee}
               + max(T_target^{l_e+1:N}, T_draft^{gen}(gamma))
               + T_rv^{fv}

    Parameters:
        target_prefix_time_ms: T_target^{1:l_e}
        target_suffix_time_ms: T_target^{l_e+1:N} (= Delta, overlap budget)
        draft_gen_time_ms: T_draft^{gen}(gamma)
        rendezvous_ee_ms: T_rv^{ee} (early-exit rendezvous)
        rendezvous_fv_ms: T_rv^{fv} (final verification rendezvous)

    Returns:
        Estimated step latency in milliseconds.
    """
    parallel_region = max(target_suffix_time_ms, draft_gen_time_ms)
    return target_prefix_time_ms + rendezvous_ee_ms + parallel_region + rendezvous_fv_ms
