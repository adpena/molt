"""
molt.gpu.generate — Text generation utilities.

Provides greedy decoding, top-k sampling, top-p (nucleus) sampling,
and temperature-controlled generation.
"""

import math
import random
from .tensor import Tensor


def greedy_decode(model, prompt_tokens, max_new_tokens=100, eos_token_id=None):
    """Generate text by always picking the highest-probability token."""
    tokens = list(prompt_tokens)
    for _ in range(max_new_tokens):
        logits = model(tokens)
        # Get logits for the last position
        if logits.ndim == 2:
            last_logits = [logits._get_row(logits.shape[0] - 1)]
        else:
            last_logits = logits.to_list()
            if isinstance(last_logits[0], list):
                last_logits = last_logits[-1]

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
