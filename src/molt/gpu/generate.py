"""
molt.gpu.generate — Text generation utilities.

Provides greedy decoding, top-k sampling, top-p (nucleus) sampling,
temperature-controlled generation, and lossless block-speculative decoding.
"""

import math
import os
import random
from .tensor import Tensor
from .dflash import (
    DFlashSelectionContext,
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeDraftResult,
    SpeculativeVerifyRequest,
    SpeculativeVerifyResult,
    resolve_dflash_runtime,
)


class SpeculativeDecodeResult:
    """Result payload for lossless block-speculative decoding."""

    def __init__(
        self,
        prompt_tokens,
        generated_tokens,
        *,
        drafted_tokens: int,
        accepted_draft_tokens: int,
        target_tokens_emitted: int,
        verify_calls: int,
    ) -> None:
        self.prompt_tokens = list(prompt_tokens)
        self.generated_tokens = list(generated_tokens)
        self.tokens = self.prompt_tokens + self.generated_tokens
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


def _requested_gpu_backend() -> str | None:
    backend = os.environ.get("MOLT_GPU_BACKEND")
    if backend is None:
        return None
    backend = backend.strip().lower()
    return backend or None


def _resolve_default_dflash_runtime(
    model,
    prompt_tokens,
    *,
    dflash_adapter: str | None = None,
    max_new_tokens: int,
    block_size: int,
    eos_token_id,
):
    context = DFlashSelectionContext(
        model=model,
        backend=_requested_gpu_backend(),
        prompt_tokens=prompt_tokens,
        eos_token_id=eos_token_id,
        max_new_tokens=max_new_tokens,
        block_size=block_size,
    )
    preferred_name = dflash_adapter
    if preferred_name is None:
        preferred_name = getattr(model, "dflash_adapter", None)
    if preferred_name is not None and not isinstance(preferred_name, str):
        raise TypeError("dflash adapter name must be a string when set")
    return resolve_dflash_runtime(
        context,
        preferred_name=preferred_name,
    )


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

    prompt = _normalize_token_sequence(prompt_tokens, "prompt_tokens")
    prefix = list(prompt)
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
            if eos_token_id is not None and target_token == eos_token_id:
                return SpeculativeDecodeResult(
                    prompt,
                    emitted,
                    drafted_tokens=drafted_total,
                    accepted_draft_tokens=accepted_total,
                    target_tokens_emitted=target_total,
                    verify_calls=verify_calls,
                )
            prefix.append(target_token)
            emitted.append(target_token)
            target_total += 1
            if len(emitted) >= max_new_tokens or mismatch:
                break

        if mismatch or len(emitted) >= max_new_tokens:
            continue

        extra_token = verified[len(drafted)]
        if eos_token_id is not None and extra_token == eos_token_id:
            break
        prefix.append(extra_token)
        emitted.append(extra_token)
        target_total += 1

    return SpeculativeDecodeResult(
        prompt,
        emitted,
        drafted_tokens=drafted_total,
        accepted_draft_tokens=accepted_total,
        target_tokens_emitted=target_total,
        verify_calls=verify_calls,
    )


def speculative_decode_greedy_conditioned(
    verify_step,
    draft_step,
    prompt_tokens,
    *,
    initial_conditioning: SpeculativeConditioning | None = None,
    max_new_tokens=100,
    block_size=16,
    eos_token_id=None,
):
    """Lossless speculative decoding with explicit verifier/drafter separation.

    The target verifier owns the conditioning payload. The drafter only
    receives that opaque payload and returns proposed tokens.
    """
    if max_new_tokens < 0:
        raise ValueError("max_new_tokens must be non-negative")
    if block_size <= 0:
        raise ValueError("block_size must be positive")

    prompt = _normalize_token_sequence(prompt_tokens, "prompt_tokens")
    prefix = list(prompt)
    emitted = []
    drafted_total = 0
    accepted_total = 0
    target_total = 0
    verify_calls = 0
    step_index = 0
    conditioning = initial_conditioning or SpeculativeConditioning()

    while len(emitted) < max_new_tokens:
        remaining = max_new_tokens - len(emitted)
        request_size = block_size if block_size < remaining else remaining

        draft_request = SpeculativeDraftRequest(
            prefix,
            request_size,
            conditioning,
            step_index=step_index,
        )
        draft_result = draft_step(draft_request)
        drafted = _normalize_token_sequence(
            draft_result.draft_tokens,
            "draft_step",
        )
        if not drafted:
            raise ValueError("draft_step must return at least one token")
        if len(drafted) > request_size:
            raise ValueError("draft_step returned more than the requested block size")
        drafted_total += len(drafted)

        verify_request = SpeculativeVerifyRequest(
            prefix,
            drafted,
            conditioning,
            step_index=step_index,
        )
        verify_result = verify_step(verify_request)
        verified = _normalize_token_sequence(
            verify_result.verified_tokens,
            "verify_step",
        )
        verify_calls += 1
        if len(verified) != len(drafted) + 1:
            raise ValueError(
                "verify_step must return len(draft_tokens) + 1 target tokens"
            )
        if verify_result.conditioning is not None:
            conditioning = verify_result.conditioning

        mismatch = False
        for idx, draft_token in enumerate(drafted):
            target_token = verified[idx]
            if draft_token == target_token:
                accepted_total += 1
            else:
                mismatch = True
            if eos_token_id is not None and target_token == eos_token_id:
                return SpeculativeDecodeResult(
                    prompt,
                    emitted,
                    drafted_tokens=drafted_total,
                    accepted_draft_tokens=accepted_total,
                    target_tokens_emitted=target_total,
                    verify_calls=verify_calls,
                )
            prefix.append(target_token)
            emitted.append(target_token)
            target_total += 1
            if len(emitted) >= max_new_tokens or mismatch:
                break

        if mismatch or len(emitted) >= max_new_tokens:
            step_index += 1
            continue

        extra_token = verified[len(drafted)]
        if eos_token_id is not None and extra_token == eos_token_id:
            break
        prefix.append(extra_token)
        emitted.append(extra_token)
        target_total += 1
        step_index += 1

    return SpeculativeDecodeResult(
        prompt,
        emitted,
        drafted_tokens=drafted_total,
        accepted_draft_tokens=accepted_total,
        target_tokens_emitted=target_total,
        verify_calls=verify_calls,
    )


def greedy_decode(
    model,
    prompt_tokens,
    max_new_tokens=100,
    eos_token_id=None,
    *,
    draft_block=None,
    verify_block=None,
    block_size=16,
    prefer_dflash: bool = True,
    dflash_adapter: str | None = None,
):
    """Generate text by always picking the highest-probability token."""
    if draft_block is not None or verify_block is not None:
        if draft_block is None or verify_block is None:
            raise ValueError(
                "greedy_decode speculative mode requires both draft_block and verify_block"
            )
        speculative = speculative_decode_greedy(
            verify_block,
            draft_block,
            prompt_tokens,
            max_new_tokens=max_new_tokens,
            block_size=block_size,
            eos_token_id=eos_token_id,
        )
        return speculative.tokens

    if prefer_dflash:
        runtime = _resolve_default_dflash_runtime(
            model,
            prompt_tokens,
            dflash_adapter=dflash_adapter,
            max_new_tokens=max_new_tokens,
            block_size=block_size,
            eos_token_id=eos_token_id,
        )
        if runtime is not None:
            speculative = speculative_decode_greedy_conditioned(
                runtime.verify_step,
                runtime.draft_step,
                prompt_tokens,
                initial_conditioning=runtime.initial_conditioning,
                max_new_tokens=max_new_tokens,
                block_size=runtime.block_size or block_size,
                eos_token_id=eos_token_id,
            )
            return speculative.tokens

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
