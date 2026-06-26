"""Generic lossless block-speculative decoding utilities.

This module is intentionally neutral: it owns generic speculative request,
response, conditioning, and decode-loop primitives. DFlash-specific contracts
live in ``molt.gpu.dflash`` and may specialize these primitives, but generic
speculative decoding must not live under the DFlash package.
"""

from __future__ import annotations

__all__ = [
    "SpeculativeConditioning",
    "SpeculativeDraftRequest",
    "SpeculativeDraftResult",
    "SpeculativeVerifyRequest",
    "SpeculativeVerifyResult",
    "SpeculativeDecodeResult",
    "speculative_decode_greedy",
    "speculative_decode_greedy_conditioned",
]


class SpeculativeConditioning:
    """Opaque target-owned conditioning payload for speculative drafters."""

    def __init__(
        self,
        *,
        target_features=None,
        target_kv=None,
        patch_features=None,
        position_ids=None,
        aux=None,
    ) -> None:
        self.target_features = target_features
        self.target_kv = target_kv
        self.patch_features = patch_features
        self.position_ids = position_ids
        self.aux = aux

    def validate_refresh_conditioning(self, conditioning, source_name: str):
        """Validate refreshed verifier-owned conditioning for this decode loop."""
        if not isinstance(conditioning, SpeculativeConditioning):
            raise TypeError(f"{source_name} must be SpeculativeConditioning")
        return conditioning


class SpeculativeDraftRequest:
    """Input to a model-specific drafter step."""

    def __init__(
        self,
        prefix_tokens,
        max_block_size: int,
        conditioning: SpeculativeConditioning,
        *,
        step_index: int,
    ) -> None:
        self.prefix_tokens = list(prefix_tokens)
        self.max_block_size = max_block_size
        self.conditioning = conditioning
        self.step_index = step_index


class SpeculativeDraftResult:
    """Draft model output: proposed tokens only."""

    def __init__(self, draft_tokens) -> None:
        self.draft_tokens = list(draft_tokens)


class SpeculativeVerifyRequest:
    """Input to a target-model verification step."""

    def __init__(
        self,
        prefix_tokens,
        draft_tokens,
        conditioning: SpeculativeConditioning,
        *,
        step_index: int,
    ) -> None:
        self.prefix_tokens = list(prefix_tokens)
        self.draft_tokens = list(draft_tokens)
        self.conditioning = conditioning
        self.step_index = step_index


class SpeculativeVerifyResult:
    """Target-model verification output with optional refreshed conditioning."""

    def __init__(
        self, verified_tokens, *, conditioning: SpeculativeConditioning | None = None
    ) -> None:
        self.verified_tokens = list(verified_tokens)
        self.conditioning = conditioning


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


def _normalize_token_id(value, error_message: str) -> int:
    if isinstance(value, bool):
        raise TypeError(error_message)
    try:
        token = int(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(error_message) from exc
    if token != value:
        raise TypeError(error_message)
    return token


def _normalize_token_sequence(values, source_name):
    out = []
    for value in values:
        out.append(
            _normalize_token_id(value, f"{source_name} must return integer token ids")
        )
    return out


def _normalize_optional_token_id(value, field_name: str) -> int | None:
    if value is None:
        return None
    return _normalize_token_id(value, f"{field_name} must be an integer token id")


def _require_non_negative_int(value, field_name: str) -> int:
    if isinstance(value, bool):
        raise TypeError(f"{field_name} must be a non-negative integer")
    try:
        number = int(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(f"{field_name} must be a non-negative integer") from exc
    if number != value:
        raise TypeError(f"{field_name} must be a non-negative integer")
    if number < 0:
        raise ValueError(f"{field_name} must be non-negative")
    return number


def _require_positive_int(value, field_name: str) -> int:
    if isinstance(value, bool):
        raise TypeError(f"{field_name} must be a positive integer")
    try:
        number = int(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(f"{field_name} must be a positive integer") from exc
    if number != value:
        raise TypeError(f"{field_name} must be a positive integer")
    if number <= 0:
        raise ValueError(f"{field_name} must be positive")
    return number


def _require_conditioning(conditioning, source_name: str) -> SpeculativeConditioning:
    if not isinstance(conditioning, SpeculativeConditioning):
        raise TypeError(f"{source_name} must be SpeculativeConditioning")
    return conditioning


def _require_draft_result(result, source_name: str) -> SpeculativeDraftResult:
    if not isinstance(result, SpeculativeDraftResult):
        raise TypeError(f"{source_name} must return SpeculativeDraftResult")
    return result


def _require_verify_result(result, source_name: str) -> SpeculativeVerifyResult:
    if not isinstance(result, SpeculativeVerifyResult):
        raise TypeError(f"{source_name} must return SpeculativeVerifyResult")
    return result


def _run_lossless_speculative_decode(
    *,
    prompt_tokens,
    max_new_tokens,
    block_size,
    eos_token_id,
    draft_tokens_fn,
    verify_tokens_fn,
    draft_source_name: str,
    verify_source_name: str,
    verified_length_name: str,
):
    max_new_tokens = _require_non_negative_int(max_new_tokens, "max_new_tokens")
    block_size = _require_positive_int(block_size, "block_size")
    eos_token_id = _normalize_optional_token_id(eos_token_id, "eos_token_id")

    prompt = _normalize_token_sequence(prompt_tokens, "prompt_tokens")
    prefix = list(prompt)
    emitted = []
    drafted_total = 0
    accepted_total = 0
    target_total = 0
    verify_calls = 0
    step_index = 0

    while len(emitted) < max_new_tokens:
        remaining = max_new_tokens - len(emitted)
        request_size = block_size if block_size < remaining else remaining

        drafted = _normalize_token_sequence(
            draft_tokens_fn(prefix, request_size, step_index),
            draft_source_name,
        )
        if not drafted:
            raise ValueError(f"{draft_source_name} must return at least one token")
        if len(drafted) > request_size:
            raise ValueError(
                f"{draft_source_name} returned more than the requested block size"
            )
        drafted_total += len(drafted)

        verified = _normalize_token_sequence(
            verify_tokens_fn(prefix, drafted, step_index),
            verify_source_name,
        )
        verify_calls += 1
        if len(verified) != len(drafted) + 1:
            raise ValueError(
                f"{verify_source_name} must return "
                f"len({verified_length_name}) + 1 target tokens"
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


def speculative_decode_greedy(
    verify_block,
    draft_block,
    prompt_tokens,
    *,
    max_new_tokens=100,
    block_size=16,
    eos_token_id=None,
):
    """Lossless block-speculative greedy decoding."""
    return _run_lossless_speculative_decode(
        prompt_tokens=prompt_tokens,
        max_new_tokens=max_new_tokens,
        block_size=block_size,
        eos_token_id=eos_token_id,
        draft_tokens_fn=lambda prefix, request_size, _step_index: draft_block(
            prefix, request_size
        ),
        verify_tokens_fn=lambda prefix, drafted, _step_index: verify_block(
            prefix, drafted
        ),
        draft_source_name="draft_block",
        verify_source_name="verify_block",
        verified_length_name="drafted_tokens",
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
    """Lossless speculative decoding with explicit verifier/drafter separation."""
    conditioning = (
        SpeculativeConditioning()
        if initial_conditioning is None
        else _require_conditioning(initial_conditioning, "initial_conditioning")
    )
    refresh_validator = conditioning.validate_refresh_conditioning
    conditioning = refresh_validator(conditioning, "initial_conditioning")

    def draft_tokens(prefix, request_size, step_index):
        draft_request = SpeculativeDraftRequest(
            prefix,
            request_size,
            conditioning,
            step_index=step_index,
        )
        draft_result = _require_draft_result(
            draft_step(draft_request),
            "draft_step",
        )
        return draft_result.draft_tokens

    def verify_tokens(prefix, drafted, step_index):
        nonlocal conditioning
        verify_request = SpeculativeVerifyRequest(
            prefix,
            drafted,
            conditioning,
            step_index=step_index,
        )
        verify_result = _require_verify_result(
            verify_step(verify_request),
            "verify_step",
        )
        if verify_result.conditioning is not None:
            conditioning = refresh_validator(
                verify_result.conditioning,
                "verify_result.conditioning",
            )
        return verify_result.verified_tokens

    return _run_lossless_speculative_decode(
        prompt_tokens=prompt_tokens,
        max_new_tokens=max_new_tokens,
        block_size=block_size,
        eos_token_id=eos_token_id,
        draft_tokens_fn=draft_tokens,
        verify_tokens_fn=verify_tokens,
        draft_source_name="draft_step",
        verify_source_name="verify_step",
        verified_length_name="draft_tokens",
    )
