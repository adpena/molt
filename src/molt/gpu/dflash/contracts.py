"""DFlash-facing speculative decoding contracts.

This module intentionally contains only generic transport objects. Target-model
verification logic and draft-model logic stay outside Molt core and communicate
through these contracts.
"""

from __future__ import annotations


class SpeculativeConditioning:
    """Opaque target-owned conditioning payload for speculative drafters."""

    def __init__(self, *, target_features=None, target_kv=None) -> None:
        self.target_features = target_features
        self.target_kv = target_kv


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
