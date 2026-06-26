"""Typed registry and resolver for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here, but every
registered adapter must identify its target model, draft model, target-layer
conditioning schema, and provenance so an anonymous generic speculative decoder
cannot enter the DFlash namespace.
"""

from __future__ import annotations

from .contracts import (
    DFlashRuntime,
    DFlashSelectionContext,
    require_dflash_draft_output_contract,
)


_DFLASH_ADAPTERS = {}
DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS = {
    "base_dflash": frozenset({"block_sequence"}),
    "dflash_v2": frozenset({"block_sequence"}),
    "dflare": frozenset({"block_sequence"}),
    "dual_diffusion_draft": frozenset({"block_sequence"}),
    "ddtree": frozenset({"per_position_marginals"}),
}
DFLASH_ALGORITHM_FAMILIES = frozenset(DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS)


def _require_non_empty_string(value, field_name: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"dflash adapter {field_name} must be a string")
    normalized = value.strip()
    if not normalized:
        raise ValueError(f"dflash adapter {field_name} must be non-empty")
    return normalized


def _require_algorithm_family(value) -> str:
    family = _require_non_empty_string(value, "algorithm_family").strip().lower()
    if family not in DFLASH_ALGORITHM_FAMILIES:
        allowed = ", ".join(sorted(DFLASH_ALGORITHM_FAMILIES))
        raise ValueError(
            f"dflash adapter algorithm_family must be a DFlash-family value: {allowed}"
        )
    return family


def _require_bool_true(value, field_name: str) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"dflash adapter {field_name} must be a bool")
    if not value:
        raise ValueError(f"dflash adapter {field_name} must be true")
    return value


def _require_token_id(value, field_name: str) -> int:
    if isinstance(value, bool):
        raise TypeError(f"dflash adapter {field_name} must be an integer token id")
    token = int(value)
    if token != value:
        raise TypeError(f"dflash adapter {field_name} must be an integer token id")
    if token < 0:
        raise ValueError(f"dflash adapter {field_name} must be non-negative")
    return token


def _require_positive_int(value, field_name: str) -> int:
    if isinstance(value, bool):
        raise TypeError(f"dflash adapter {field_name} must be a positive integer")
    number = int(value)
    if number != value:
        raise TypeError(f"dflash adapter {field_name} must be a positive integer")
    if number <= 0:
        raise ValueError(f"dflash adapter {field_name} must be positive")
    return number


def _require_layer_ids(value) -> tuple[int, ...]:
    try:
        layer_ids = tuple(value)
    except TypeError as exc:
        raise TypeError("dflash adapter target_layer_ids must be iterable") from exc
    if not layer_ids:
        raise ValueError("dflash adapter target_layer_ids must be non-empty")
    normalized = []
    for layer_id in layer_ids:
        if isinstance(layer_id, bool):
            raise TypeError("dflash adapter target_layer_ids must contain integers")
        number = int(layer_id)
        if number != layer_id:
            raise TypeError("dflash adapter target_layer_ids must contain integers")
        if number < 0:
            raise ValueError("dflash adapter target_layer_ids must be non-negative")
        normalized.append(number)
    return tuple(normalized)


class DFlashAdapterMetadata:
    """Immutable identity metadata for a trained DFlash-family adapter.

    The serving/training ecosystem now treats block size, target layer IDs,
    hidden-state schema, KV/cross-attention schema, tokenizer, and mask-token
    identity as part of the checkpoint contract. Keeping those facts typed here
    prevents a generic speculative adapter from entering the registry with only
    a model pair and a hopeful name.
    """

    def __init__(
        self,
        *,
        algorithm_family: str,
        adapter_version: str,
        tokenizer_id: str,
        mask_token_id: int,
        target_layer_ids,
        target_feature_schema: str,
        kv_schema: str,
        target_conditioning_path: str,
        draft_output_contract: str,
        max_block_size: int,
        uses_non_causal_draft_attention: bool,
        injects_target_context_each_layer: bool,
    ) -> None:
        self.algorithm_family = _require_algorithm_family(algorithm_family)
        self.adapter_version = _require_non_empty_string(
            adapter_version, "adapter_version"
        )
        self.tokenizer_id = _require_non_empty_string(tokenizer_id, "tokenizer_id")
        self.mask_token_id = _require_token_id(mask_token_id, "mask_token_id")
        self.target_layer_ids = _require_layer_ids(target_layer_ids)
        self.target_feature_schema = _require_non_empty_string(
            target_feature_schema, "target_feature_schema"
        )
        self.kv_schema = _require_non_empty_string(kv_schema, "kv_schema")
        self.target_conditioning_path = _require_non_empty_string(
            target_conditioning_path, "target_conditioning_path"
        )
        self.draft_output_contract = require_dflash_draft_output_contract(
            draft_output_contract,
            "dflash adapter draft_output_contract",
        )
        allowed_output_contracts = DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS[
            self.algorithm_family
        ]
        if self.draft_output_contract not in allowed_output_contracts:
            allowed = ", ".join(sorted(allowed_output_contracts))
            raise ValueError(
                "dflash adapter draft_output_contract is incompatible with "
                f"algorithm_family {self.algorithm_family}: {allowed}"
            )
        self.max_block_size = _require_positive_int(max_block_size, "max_block_size")
        if self.max_block_size == 1:
            raise ValueError(
                "dflash adapter max_block_size must support block drafting"
            )
        self.uses_non_causal_draft_attention = _require_bool_true(
            uses_non_causal_draft_attention, "uses_non_causal_draft_attention"
        )
        self.injects_target_context_each_layer = _require_bool_true(
            injects_target_context_each_layer, "injects_target_context_each_layer"
        )


class DFlashAdapterSpec:
    """Typed adapter record for target/draft-model-specific DFlash integrations."""

    def __init__(
        self,
        *,
        name: str,
        target_model_id: str,
        draft_model_id: str,
        provenance: str,
        metadata: DFlashAdapterMetadata,
        supports,
        create_runtime,
        priority: int = 0,
    ) -> None:
        self.name = _require_non_empty_string(name, "name")
        self.target_model_id = _require_non_empty_string(
            target_model_id, "target_model_id"
        )
        self.draft_model_id = _require_non_empty_string(
            draft_model_id, "draft_model_id"
        )
        self.provenance = _require_non_empty_string(provenance, "provenance")
        if not isinstance(metadata, DFlashAdapterMetadata):
            raise TypeError("dflash adapter metadata must be DFlashAdapterMetadata")
        self.metadata = metadata
        if not callable(supports):
            raise TypeError("dflash adapter supports must be callable")
        if not callable(create_runtime):
            raise TypeError("dflash adapter create_runtime must be callable")
        self.supports = supports
        self.create_runtime = create_runtime
        self.priority = priority


def _adapter_metadata_matches_context(spec: DFlashAdapterSpec, context) -> bool:
    if not isinstance(context, DFlashSelectionContext):
        raise TypeError("dflash adapter context must be DFlashSelectionContext")
    if context.target_model_id != spec.target_model_id:
        return False
    if context.tokenizer_id != spec.metadata.tokenizer_id:
        return False
    if context.block_size == 1:
        return False
    return context.block_size <= spec.metadata.max_block_size


def _adapter_supports(spec: DFlashAdapterSpec, context) -> bool:
    if not _adapter_metadata_matches_context(spec, context):
        return False
    supported = spec.supports(context)
    if not isinstance(supported, bool):
        raise TypeError("dflash adapter supports() must return bool")
    return supported


def register_dflash_adapter(spec: DFlashAdapterSpec) -> None:
    if not isinstance(spec, DFlashAdapterSpec):
        raise TypeError("register_dflash_adapter expects DFlashAdapterSpec")
    if spec.name in _DFLASH_ADAPTERS:
        raise ValueError(f"dflash adapter '{spec.name}' is already registered")
    _DFLASH_ADAPTERS[spec.name] = spec


def get_dflash_adapter(name: str):
    return _DFLASH_ADAPTERS.get(name)


def list_dflash_adapters():
    return sorted(_DFLASH_ADAPTERS)


def snapshot_dflash_adapters():
    """Return a shallow snapshot of the registered adapter specs.

    Adapter specs are immutable by convention after registration. The snapshot
    is intended for deterministic registry lifecycle management in embedding
    contexts and tests that need temporary adapter sets.
    """
    return dict(_DFLASH_ADAPTERS)


def restore_dflash_adapters(snapshot) -> None:
    if not isinstance(snapshot, dict):
        raise TypeError("dflash adapter snapshot must be a dict")
    restored = {}
    for name, spec in snapshot.items():
        if not isinstance(name, str) or not name:
            raise ValueError("dflash adapter snapshot keys must be non-empty strings")
        if not isinstance(spec, DFlashAdapterSpec):
            raise TypeError("dflash adapter snapshot values must be DFlashAdapterSpec")
        if spec.name != name:
            raise ValueError("dflash adapter snapshot key must match spec.name")
        restored[name] = spec
    _DFLASH_ADAPTERS.clear()
    _DFLASH_ADAPTERS.update(restored)


def clear_dflash_adapters() -> None:
    _DFLASH_ADAPTERS.clear()


def has_dflash_backend(backend: str | None) -> bool:
    if backend is None:
        return False
    return backend.strip() != ""


def resolve_dflash_adapter(context, preferred_name: str | None = None):
    backend = context.backend
    if not has_dflash_backend(backend):
        return None
    if preferred_name is None:
        return None

    adapter = get_dflash_adapter(preferred_name)
    if adapter is None:
        return None
    return adapter if _adapter_supports(adapter, context) else None


def resolve_default_dflash_adapter(context):
    if not has_dflash_backend(context.backend):
        return None
    candidates = []
    for name in list_dflash_adapters():
        adapter = get_dflash_adapter(name)
        if adapter is not None and _adapter_supports(adapter, context):
            candidates.append(adapter)
    if not candidates:
        return None
    candidates.sort(key=lambda adapter: (-adapter.priority, adapter.name))
    top = candidates[0]
    tied = [adapter for adapter in candidates if adapter.priority == top.priority]
    if len(tied) > 1:
        names = ", ".join(adapter.name for adapter in tied)
        raise ValueError(
            f"multiple dflash adapters match with the same priority: {names}"
        )
    return top


def resolve_dflash_runtime(context, preferred_name: str | None = None):
    if preferred_name is None:
        adapter = resolve_default_dflash_adapter(context)
    else:
        adapter = resolve_dflash_adapter(context, preferred_name=preferred_name)
    if adapter is None:
        return None
    runtime = adapter.create_runtime(context)
    if not isinstance(runtime, DFlashRuntime):
        raise TypeError("dflash adapter create_runtime() must return DFlashRuntime")
    effective_block_size = runtime.block_size or context.block_size
    if effective_block_size == 1:
        raise ValueError("dflash runtime block_size must support block drafting")
    if effective_block_size > adapter.metadata.max_block_size:
        raise ValueError(
            "dflash runtime block_size exceeds adapter metadata max_block_size"
        )
    if runtime.draft_output_contract != adapter.metadata.draft_output_contract:
        raise ValueError(
            "dflash runtime draft_output_contract does not match adapter metadata"
        )
    return runtime


def build_dflash_runtime(
    model,
    prompt_tokens,
    *,
    backend: str | None,
    dflash_adapter: str | None = None,
    eos_token_id=None,
    max_new_tokens: int = 100,
    block_size: int = 16,
    target_model_id: str,
    tokenizer_id: str,
    adapter_payload=None,
):
    context = DFlashSelectionContext(
        model=model,
        backend=backend,
        prompt_tokens=prompt_tokens,
        eos_token_id=eos_token_id,
        max_new_tokens=max_new_tokens,
        block_size=block_size,
        target_model_id=target_model_id,
        tokenizer_id=tokenizer_id,
        adapter_payload=adapter_payload,
    )
    runtime = resolve_dflash_runtime(context, preferred_name=dflash_adapter)
    if runtime is not None:
        return runtime
    if dflash_adapter is not None:
        raise LookupError(
            f"dflash adapter '{dflash_adapter}' is unavailable for this context"
        )
    return None
