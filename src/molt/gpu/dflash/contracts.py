"""DFlash-facing speculative decoding contracts.

This module intentionally contains only generic transport objects. Target-model
verification logic and draft-model logic stay outside Molt core and communicate
through these contracts.
"""

from __future__ import annotations


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

    def __init__(self, verified_tokens, *, conditioning: SpeculativeConditioning | None = None) -> None:
        self.verified_tokens = list(verified_tokens)
        self.conditioning = conditioning


class DFlashRuntime:
    """Fully bound speculative runtime produced by a model-specific adapter."""

    def __init__(
        self,
        *,
        draft_step,
        verify_step,
        initial_conditioning: SpeculativeConditioning | None = None,
        block_size: int | None = None,
    ) -> None:
        self.draft_step = draft_step
        self.verify_step = verify_step
        self.initial_conditioning = initial_conditioning
        self.block_size = block_size


class DFlashSelectionContext:
    """Selection-time context for choosing and instantiating an adapter."""

    def __init__(
        self,
        *,
        model,
        backend: str | None,
        prompt_tokens,
        eos_token_id,
        max_new_tokens: int,
        block_size: int,
        adapter_payload=None,
    ) -> None:
        self.model = model
        self.backend = backend
        self.prompt_tokens = list(prompt_tokens)
        self.eos_token_id = eos_token_id
        self.max_new_tokens = max_new_tokens
        self.block_size = block_size
        self.adapter_payload = adapter_payload
