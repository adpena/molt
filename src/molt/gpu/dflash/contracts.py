"""DFlash-facing speculative decoding contracts.

Provenance:
- Chen, Liang, and Liu, "DFlash: Block Diffusion for Flash Speculative
  Decoding", arXiv:2602.06036, https://arxiv.org/abs/2602.06036
- Official DFlash project, https://z-lab.ai/projects/dflash/

The paper/project define DFlash as target-conditioned block-diffusion
drafting. Target hidden features are fused and injected into each draft
layer's KV cache; drafting is conditioned on that target context and the last
verified token. Molt core keeps target-model verification logic and
draft-model logic outside this module, but the transport contract below is
strict enough to prevent generic speculative decoding from being mislabeled as
DFlash.
"""

from __future__ import annotations

from ..speculative import SpeculativeConditioning


class DFlashConditioning(SpeculativeConditioning):
    """Required target-conditioned payload for a paper-faithful DFlash drafter.

    DFlash drafting is not generic speculative decoding: the drafter must be
    conditioned on target-model features/KV state and the last verified token.
    """

    def __init__(
        self,
        *,
        target_features,
        target_kv,
        position_ids,
        last_verified_token: int,
        patch_features=None,
        aux=None,
    ) -> None:
        if target_features is None:
            raise ValueError("DFlashConditioning requires target_features")
        if target_kv is None:
            raise ValueError("DFlashConditioning requires target_kv")
        if position_ids is None:
            raise ValueError("DFlashConditioning requires position_ids")
        if isinstance(last_verified_token, bool):
            raise TypeError("last_verified_token must be an integer token id")
        token = int(last_verified_token)
        if token != last_verified_token:
            raise TypeError("last_verified_token must be an integer token id")
        super().__init__(
            target_features=target_features,
            target_kv=target_kv,
            patch_features=patch_features,
            position_ids=list(position_ids),
            aux=aux,
        )
        self.last_verified_token = token

    def validate_refresh_conditioning(self, conditioning, source_name: str):
        return require_dflash_conditioning(conditioning, source_name)


def require_dflash_conditioning(conditioning, source_name: str = "conditioning"):
    if not isinstance(conditioning, DFlashConditioning):
        raise TypeError(f"{source_name} must be DFlashConditioning")
    if conditioning.target_features is None:
        raise ValueError(f"{source_name} requires target_features")
    if conditioning.target_kv is None:
        raise ValueError(f"{source_name} requires target_kv")
    if conditioning.position_ids is None:
        raise ValueError(f"{source_name} requires position_ids")
    if not hasattr(conditioning, "last_verified_token"):
        raise ValueError(f"{source_name} requires last_verified_token")
    return conditioning


class DFlashRuntime:
    """Fully bound speculative runtime produced by a model-specific adapter."""

    def __init__(
        self,
        *,
        draft_step,
        verify_step,
        initial_conditioning: DFlashConditioning,
        block_size: int | None = None,
    ) -> None:
        if not callable(draft_step):
            raise TypeError("DFlashRuntime draft_step must be callable")
        if not callable(verify_step):
            raise TypeError("DFlashRuntime verify_step must be callable")
        if draft_step is verify_step:
            raise TypeError(
                "DFlashRuntime draft_step and verify_step must be distinct callables"
            )
        require_dflash_conditioning(initial_conditioning, "initial_conditioning")
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
