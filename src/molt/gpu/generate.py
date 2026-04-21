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
    SpeculativeDecodeResult,
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeDraftResult,
    SpeculativeVerifyRequest,
    SpeculativeVerifyResult,
    speculative_decode_greedy,
    speculative_decode_greedy_conditioned,
    has_dflash_backend,
    resolve_dflash_runtime,
)


def _requested_gpu_backend() -> str | None:
    backend = os.environ.get("MOLT_GPU_BACKEND")
    if backend is None:
        return None
    backend = backend.strip().lower()
    return backend or None


def _dflash_missing_message(adapter_name: str | None = None) -> str:
    if adapter_name:
        return f"dflash adapter '{adapter_name}' is unavailable for this context"
    return (
        "no dflash adapter is available for this context; DFlash requires a "
        "model-specific trained drafter/verifier adapter"
    )


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
    runtime = resolve_dflash_runtime(
        context,
        preferred_name=preferred_name,
    )
    if runtime is None and preferred_name is not None and has_dflash_backend(context.backend):
        raise LookupError(_dflash_missing_message(preferred_name))
    return runtime




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
        backend = _requested_gpu_backend()
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
        if has_dflash_backend(backend):
            raise LookupError(_dflash_missing_message())

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
