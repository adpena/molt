"""Runtime orchestration for DFlash-style speculative decoding."""

from __future__ import annotations

from .contracts import (
    DFlashConditioning,
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeVerifyRequest,
    require_dflash_conditioning,
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
    """Lossless speculative decoding with explicit verifier/drafter separation."""
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
    enforce_dflash_conditioning = isinstance(conditioning, DFlashConditioning)
    if enforce_dflash_conditioning:
        require_dflash_conditioning(conditioning, "initial_conditioning")

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
            if enforce_dflash_conditioning:
                require_dflash_conditioning(
                    verify_result.conditioning,
                    "verify_result.conditioning",
                )
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
