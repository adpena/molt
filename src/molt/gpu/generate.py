"""
molt.gpu.generate — Text generation utilities.

Provides greedy decoding, top-k sampling, top-p (nucleus) sampling,
temperature-controlled generation, and lossless block-speculative decoding.
"""

import math
import random
from .tensor import Tensor


class SpeculativeDecodeResult:
    """Result payload for lossless block-speculative decoding."""

    def __init__(
        self,
        tokens,
        *,
        drafted_tokens: int,
        accepted_draft_tokens: int,
        target_tokens_emitted: int,
        verify_calls: int,
    ) -> None:
        self.tokens = list(tokens)
        self.drafted_tokens = drafted_tokens
        self.accepted_draft_tokens = accepted_draft_tokens
        self.target_tokens_emitted = target_tokens_emitted
        self.verify_calls = verify_calls

    @property
    def acceptance_rate(self) -> float:
        if self.drafted_tokens == 0:
            return 0.0
        return float(self.accepted_draft_tokens) / float(self.drafted_tokens)


def _normalize_token_sequence(values, source_name):
    out = []
    for value in values:
        if isinstance(value, bool):
            raise TypeError(f"{source_name} must return integer token ids")
        token = int(value)
        if token != value:
            raise TypeError(f"{source_name} must return integer token ids")
        out.append(token)
    return out


def speculative_decode_greedy(
    verify_block,
    draft_block,
    prompt_tokens,
    *,
    max_new_tokens=100,
    block_size=16,
    eos_token_id=None,
):
    """Lossless block-speculative greedy decoding.

    ``draft_block(prefix_tokens, requested_block_size)`` must return a proposed
    block of at least one and at most ``requested_block_size`` token ids.

    ``verify_block(prefix_tokens, drafted_tokens)`` must return the target
    model's greedy next-token ids for each drafted position plus one extra next
    token, i.e. ``len(drafted_tokens) + 1`` ids total.

    This is the generic native verification loop needed by DFlash-style block
    drafters, without assuming any specific draft-model architecture.
    """
    if max_new_tokens < 0:
        raise ValueError("max_new_tokens must be non-negative")
    if block_size <= 0:
        raise ValueError("block_size must be positive")

    prefix = _normalize_token_sequence(prompt_tokens, "prompt_tokens")
    emitted = []
    drafted_total = 0
    accepted_total = 0
    target_total = 0
    verify_calls = 0

    while len(emitted) < max_new_tokens:
        remaining = max_new_tokens - len(emitted)
        request_size = block_size if block_size < remaining else remaining

        drafted = _normalize_token_sequence(
            draft_block(prefix, request_size),
            "draft_block",
        )
        if not drafted:
            raise ValueError("draft_block must return at least one token")
        if len(drafted) > request_size:
            raise ValueError("draft_block returned more than the requested block size")
        drafted_total += len(drafted)

        verified = _normalize_token_sequence(
            verify_block(prefix, drafted),
            "verify_block",
        )
        verify_calls += 1
        if len(verified) != len(drafted) + 1:
            raise ValueError(
                "verify_block must return len(drafted_tokens) + 1 target tokens"
            )

        mismatch = False
        for idx, draft_token in enumerate(drafted):
            target_token = verified[idx]
            if draft_token == target_token:
                accepted_total += 1
            else:
                mismatch = True
            prefix.append(target_token)
            emitted.append(target_token)
            target_total += 1
            if eos_token_id is not None and target_token == eos_token_id:
                return SpeculativeDecodeResult(
                    emitted,
                    drafted_tokens=drafted_total,
                    accepted_draft_tokens=accepted_total,
                    target_tokens_emitted=target_total,
                    verify_calls=verify_calls,
                )
            if len(emitted) >= max_new_tokens or mismatch:
                break

        if mismatch or len(emitted) >= max_new_tokens:
            continue

        extra_token = verified[len(drafted)]
        prefix.append(extra_token)
        emitted.append(extra_token)
        target_total += 1
        if eos_token_id is not None and extra_token == eos_token_id:
            break

    return SpeculativeDecodeResult(
        emitted,
        drafted_tokens=drafted_total,
        accepted_draft_tokens=accepted_total,
        target_tokens_emitted=target_total,
        verify_calls=verify_calls,
    )


def greedy_decode(model, prompt_tokens, max_new_tokens=100, eos_token_id=None):
    """Generate text by always picking the highest-probability token."""
    tokens = list(prompt_tokens)
    for _ in range(max_new_tokens):
        logits = model(tokens)
        # Get logits for the last position
        last_logits = get_last_logits(logits)

        next_token = argmax(last_logits)
        if eos_token_id is not None and next_token == eos_token_id:
            break
        tokens.append(next_token)
    return tokens


def top_k_sample(model, prompt_tokens, max_new_tokens=100, k=50, temperature=1.0, eos_token_id=None):
    """Generate with top-k sampling: only consider the k highest-probability tokens."""
    tokens = list(prompt_tokens)
    for _ in range(max_new_tokens):
        logits = model(tokens)
        last_logits = get_last_logits(logits)

        # temperature=0 or None → greedy (argmax)
        if temperature is None or temperature == 0:
            next_token = argmax(last_logits)
        else:
            # Apply temperature
            if temperature != 1.0:
                last_logits = [l / temperature for l in last_logits]

            # Top-k: zero out everything except top k
            indexed = sorted(enumerate(last_logits), key=lambda x: -x[1])
            top_k_indices = set(i for i, _ in indexed[:k])
            filtered = [l if i in top_k_indices else -1e9 for i, l in enumerate(last_logits)]

            # Softmax + sample
            probs = softmax_list(filtered)
            next_token = sample_from_probs(probs)

        if eos_token_id is not None and next_token == eos_token_id:
            break
        tokens.append(next_token)
    return tokens


def top_p_sample(model, prompt_tokens, max_new_tokens=100, p=0.9, temperature=1.0, eos_token_id=None):
    """Generate with nucleus (top-p) sampling: sample from the smallest set whose probability >= p."""
    tokens = list(prompt_tokens)
    for _ in range(max_new_tokens):
        logits = model(tokens)
        last_logits = get_last_logits(logits)

        # temperature=0 or None → greedy (argmax)
        if temperature is None or temperature == 0:
            next_token = argmax(last_logits)
        else:
            if temperature != 1.0:
                last_logits = [l / temperature for l in last_logits]

            # Sort by probability
            probs = softmax_list(last_logits)
            indexed = sorted(enumerate(probs), key=lambda x: -x[1])

            # Accumulate until we reach p
            cumsum = 0.0
            allowed = set()
            for idx, prob in indexed:
                cumsum += prob
                allowed.add(idx)
                if cumsum >= p:
                    break

            # Zero out non-allowed, re-normalize
            filtered_probs = [prob if i in allowed else 0.0 for i, prob in enumerate(probs)]
            total = sum(filtered_probs)
            if total > 0:
                filtered_probs = [fp / total for fp in filtered_probs]

            next_token = sample_from_probs(filtered_probs)

        if eos_token_id is not None and next_token == eos_token_id:
            break
        tokens.append(next_token)
    return tokens


# --- Helpers ---

def get_last_logits(logits):
    """Extract logits for the last token position."""
    data = logits.to_list()
    if isinstance(data, list) and data and isinstance(data[0], list):
        return data[-1]
    return data


def argmax(values):
    return max(range(len(values)), key=lambda i: values[i])


def softmax_list(values):
    max_val = max(values)
    exps = [math.exp(v - max_val) for v in values]
    total = sum(exps)
    return [e / total for e in exps]


def sample_from_probs(probs):
    r = random.random()
    cumsum = 0.0
    for i, p in enumerate(probs):
        cumsum += p
        if cumsum >= r:
            return i
    return len(probs) - 1
