"""DFlash fail-closed fidelity corpus (doc 67 Phase 5b; §3.5.2).

This corpus makes the EXISTING-but-untested DFlash contract guards executable
and gated. It exercises the real typed refusals in
``src/molt/gpu/dflash/{contracts,adapters}.py`` and the mislabel guard in
``src/molt/stdlib/tinygrad/dflash.py`` so that the constitutional invariant —
"generic speculative decoding mislabeled DFlash is UNEXPRESSIBLE"
(CLAUDE.md "Top Priority: Tinygrad + DFlash Fidelity") — is held by a RED gate,
not by prose.

Every test asserts BOTH the exact exception type AND the exact molt message
text. That is deliberate: if a future change weakens a guard (e.g. drops a
required conditioning field, accepts a generic adapter, or replaces the
fail-closed ``ImportError`` with a silent fallback), the corresponding test must
FAIL LOUDLY rather than silently pass. The message-text assertions are what turn
"the guard was removed" and "the guard now raises a generic error" into RED.

These are the F3/F6 obligations (and the transport-contract half of F1/F2/F4)
from ``src/molt/gpu/dflash/SPEC.md`` — the parts gated TODAY. The F1/F2/F4/F5
*algorithm* obligations (the drafter actually consuming the conditioning, KV
injection, block diffusion, end-to-end losslessness) are Phase 5a and require the
build-heavy reference model under ``tests/gpu/dflash/reference/`` run through
``tools/safe_run.py``; they are intentionally NOT in this pure-Python corpus.

No build / no cargo: the guards under test are pure Python.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from molt.gpu.dflash import (
    DFLASH_ALGORITHM_FAMILIES,
    DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS,
    DFLASH_DRAFT_OUTPUT_CONTRACTS,
    DFlashAdapterMetadata,
    DFlashAdapterSpec,
    DFlashConditioning,
    DFlashRuntime,
    DFlashSelectionContext,
    build_dflash_runtime,
    clear_dflash_adapters,
    register_dflash_adapter,
    require_dflash_conditioning,
    resolve_dflash_runtime,
    snapshot_dflash_adapters,
    restore_dflash_adapters,
)
from molt.gpu.dflash.contracts import DFlashSelectionContext as _CtxAlias  # noqa: F401
from molt.gpu.speculative import SpeculativeConditioning


# --- shared fixtures: a *valid* DFlash conditioning, so each negative test ---
# --- isolates exactly one missing/invalid field (no false green from a -------
# --- second unrelated failure). ---------------------------------------------


def _valid_conditioning_kwargs() -> dict:
    """Kwargs that construct a fully-valid DFlashConditioning.

    Each fail-closed test below mutates exactly ONE of these to the invalid
    value it is probing, proving the guard fires on that specific field rather
    than on an incidental second problem.
    """
    return {
        "target_features": [[0.0, 1.0]],
        "target_kv": [[0.0, 0.0]],
        "position_ids": [0],
        "last_verified_token": 7,
    }


def _valid_conditioning() -> DFlashConditioning:
    return DFlashConditioning(**_valid_conditioning_kwargs())


@pytest.fixture(autouse=True)
def _isolated_adapter_registry():
    """Run each test against a clean, restored adapter registry.

    The registry is process-global module state; snapshot/restore keeps these
    tests hermetic and order-independent without mutating any other suite's
    registered adapters.
    """
    snapshot = snapshot_dflash_adapters()
    clear_dflash_adapters()
    try:
        yield
    finally:
        restore_dflash_adapters(snapshot)


# === (a) DFlashConditioning / DFlashRuntime: missing or invalid conditioning ===
# Exercises contracts.py require_dflash_conditioning + DFlashConditioning ctor +
# DFlashRuntime ctor. SPEC.md F1/F2/F3/F4/F6.


def test_conditioning_missing_target_features_raises_valueerror():
    kwargs = _valid_conditioning_kwargs()
    kwargs["target_features"] = None
    with pytest.raises(ValueError, match="DFlashConditioning requires target_features"):
        DFlashConditioning(**kwargs)


def test_conditioning_missing_target_kv_raises_valueerror():
    kwargs = _valid_conditioning_kwargs()
    kwargs["target_kv"] = None
    with pytest.raises(ValueError, match="DFlashConditioning requires target_kv"):
        DFlashConditioning(**kwargs)


def test_conditioning_missing_position_ids_raises_valueerror():
    kwargs = _valid_conditioning_kwargs()
    kwargs["position_ids"] = None
    with pytest.raises(ValueError, match="DFlashConditioning requires position_ids"):
        DFlashConditioning(**kwargs)


def test_conditioning_bool_last_verified_token_raises_typeerror():
    # bool is a subclass of int in Python; DFlash rejects it explicitly so a
    # truthy flag can never masquerade as token id 1 (SPEC.md F4).
    kwargs = _valid_conditioning_kwargs()
    kwargs["last_verified_token"] = True
    with pytest.raises(
        TypeError, match="last_verified_token must be an integer token id"
    ):
        DFlashConditioning(**kwargs)


def test_conditioning_nonintegral_last_verified_token_raises_typeerror():
    kwargs = _valid_conditioning_kwargs()
    kwargs["last_verified_token"] = 1.5
    with pytest.raises(
        TypeError, match="last_verified_token must be an integer token id"
    ):
        DFlashConditioning(**kwargs)


def test_valid_conditioning_constructs_and_normalizes():
    # Positive control: the valid kwargs really do build, so the negative tests
    # above isolate the single mutated field rather than a broken baseline.
    cond = _valid_conditioning()
    assert isinstance(cond, DFlashConditioning)
    assert isinstance(cond, SpeculativeConditioning)
    assert cond.last_verified_token == 7
    # position_ids is defensively copied to a list.
    assert cond.position_ids == [0]


def test_require_dflash_conditioning_rejects_generic_conditioning():
    # A *generic* SpeculativeConditioning is the canonical "generic speculative
    # decoding" payload. It must be rejected at the boundary (SPEC.md F3/F6).
    generic = SpeculativeConditioning(
        target_features=[[0.0]],
        target_kv=[[0.0]],
        position_ids=[0],
    )
    with pytest.raises(TypeError, match="must be DFlashConditioning"):
        require_dflash_conditioning(generic, "initial_conditioning")


def test_require_dflash_conditioning_accepts_valid_dflash_conditioning():
    cond = _valid_conditioning()
    assert require_dflash_conditioning(cond, "initial_conditioning") is cond


def test_runtime_noncallable_draft_step_raises_typeerror():
    with pytest.raises(TypeError, match="DFlashRuntime draft_step must be callable"):
        DFlashRuntime(
            draft_step=object(),
            verify_step=lambda req: req,
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="block_sequence",
        )


def test_runtime_noncallable_verify_step_raises_typeerror():
    with pytest.raises(TypeError, match="DFlashRuntime verify_step must be callable"):
        DFlashRuntime(
            draft_step=lambda req: req,
            verify_step=object(),
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="block_sequence",
        )


def test_runtime_same_draft_and_verify_callable_raises_typeerror():
    def collapsed_step(req):
        return req

    with pytest.raises(
        TypeError,
        match="DFlashRuntime draft_step and verify_step must be distinct callables",
    ):
        DFlashRuntime(
            draft_step=collapsed_step,
            verify_step=collapsed_step,
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="block_sequence",
        )


def test_runtime_generic_conditioning_raises_typeerror():
    # Even with valid callables, a generic (non-DFlash) conditioning cannot
    # produce a DFlashRuntime — the runtime is target-conditioned by contract.
    generic = SpeculativeConditioning(
        target_features=[[0.0]],
        target_kv=[[0.0]],
        position_ids=[0],
    )
    with pytest.raises(
        TypeError, match="initial_conditioning must be DFlashConditioning"
    ):
        DFlashRuntime(
            draft_step=lambda req: req,
            verify_step=lambda req: req,
            initial_conditioning=generic,
            draft_output_contract="block_sequence",
        )


def test_runtime_none_conditioning_raises_typeerror():
    # None is the degenerate "no conditioning at all" case; isinstance(None,
    # DFlashConditioning) is False so require_dflash_conditioning rejects it.
    with pytest.raises(TypeError, match="must be DFlashConditioning"):
        DFlashRuntime(
            draft_step=lambda req: req,
            verify_step=lambda req: req,
            initial_conditioning=None,
            draft_output_contract="block_sequence",
        )


def test_runtime_with_valid_inputs_constructs():
    # Positive control for the runtime path.
    rt = DFlashRuntime(
        draft_step=lambda req: req,
        verify_step=lambda req: req,
        initial_conditioning=_valid_conditioning(),
        draft_output_contract="block_sequence",
        block_size=8,
    )
    assert callable(rt.draft_step)
    assert callable(rt.verify_step)
    assert isinstance(rt.initial_conditioning, DFlashConditioning)
    assert rt.draft_output_contract == "block_sequence"
    assert rt.block_size == 8


def test_runtime_rejects_non_linear_draft_output_contract():
    with pytest.raises(
        ValueError,
        match="DFlashRuntime draft_output_contract must be block_sequence",
    ):
        DFlashRuntime(
            draft_step=lambda req: req,
            verify_step=lambda req: req,
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="per_position_marginals",
            block_size=8,
        )


# === (b) adapters: a generic, non-target-conditioned adapter under the =========
# DFlash name must not resolve to a runtime. Exercises adapters.py typed
# registry. SPEC.md F6.


def _ctx(name: str = "synthetic", backend: str = "native") -> DFlashSelectionContext:
    return DFlashSelectionContext(
        model=object(),
        backend=backend,
        prompt_tokens=[1, 2, 3],
        eos_token_id=None,
        max_new_tokens=8,
        block_size=4,
        target_model_id=f"test://target/{name}",
        tokenizer_id=f"test://tokenizer/{name}",
    )


def _adapter_spec(
    *,
    name: str,
    supports,
    create_runtime,
    priority: int = 0,
    metadata: DFlashAdapterMetadata | None = None,
) -> DFlashAdapterSpec:
    return DFlashAdapterSpec(
        name=name,
        target_model_id=f"test://target/{name}",
        draft_model_id=f"test://draft/{name}",
        provenance="test-only synthetic DFlash adapter fixture",
        metadata=metadata or _adapter_metadata(name),
        supports=supports,
        create_runtime=create_runtime,
        priority=priority,
    )


def _adapter_metadata(
    name: str = "synthetic",
    *,
    algorithm_family: str = "base_dflash",
    draft_output_contract: str = "block_sequence",
) -> DFlashAdapterMetadata:
    return DFlashAdapterMetadata(
        algorithm_family=algorithm_family,
        adapter_version=f"test://adapter-version/{name}",
        tokenizer_id=f"test://tokenizer/{name}",
        mask_token_id=0,
        target_layer_ids=[0, 2],
        target_feature_schema="test:hidden_states[batch,seq,hidden]",
        kv_schema="test:kv[layer,batch,heads,seq,dim]",
        target_conditioning_path="kv_injection_each_draft_layer",
        draft_output_contract=draft_output_contract,
        max_block_size=4,
        uses_non_causal_draft_attention=True,
        injects_target_context_each_layer=True,
    )


def test_register_dflash_adapter_rejects_non_spec():
    # The registry is typed: only DFlashAdapterSpec instances register. A bare
    # callable (a "generic adapter") cannot be smuggled in.
    with pytest.raises(
        TypeError, match="register_dflash_adapter expects DFlashAdapterSpec"
    ):
        register_dflash_adapter(object())


@pytest.mark.parametrize(
    ("field_name", "field_value", "exc_type", "message"),
    (
        (
            "target_model_id",
            "",
            ValueError,
            "dflash adapter target_model_id must be non-empty",
        ),
        (
            "draft_model_id",
            "   ",
            ValueError,
            "dflash adapter draft_model_id must be non-empty",
        ),
        (
            "provenance",
            None,
            TypeError,
            "dflash adapter provenance must be a string",
        ),
    ),
)
def test_adapter_spec_requires_model_pair_provenance(
    field_name, field_value, exc_type, message
):
    kwargs = {
        "name": "metadata-required",
        "target_model_id": "test://target/metadata-required",
        "draft_model_id": "test://draft/metadata-required",
        "provenance": "test-only synthetic DFlash adapter fixture",
        "metadata": _adapter_metadata("metadata-required"),
        "supports": lambda _context: True,
        "create_runtime": lambda _context: object(),
    }
    kwargs[field_name] = field_value
    with pytest.raises(exc_type, match=message):
        DFlashAdapterSpec(**kwargs)


@pytest.mark.parametrize(
    ("field_name", "field_value", "exc_type", "message"),
    (
        (
            "tokenizer_id",
            "",
            ValueError,
            "dflash adapter tokenizer_id must be non-empty",
        ),
        (
            "mask_token_id",
            True,
            TypeError,
            "dflash adapter mask_token_id must be an integer token id",
        ),
        (
            "target_layer_ids",
            [],
            ValueError,
            "dflash adapter target_layer_ids must be non-empty",
        ),
        (
            "target_feature_schema",
            " ",
            ValueError,
            "dflash adapter target_feature_schema must be non-empty",
        ),
        (
            "kv_schema",
            None,
            TypeError,
            "dflash adapter kv_schema must be a string",
        ),
        (
            "draft_output_contract",
            "tree_attention",
            ValueError,
            "dflash adapter draft_output_contract must be a DFlash draft output contract",
        ),
        (
            "max_block_size",
            1,
            ValueError,
            "dflash adapter max_block_size must support block drafting",
        ),
        (
            "uses_non_causal_draft_attention",
            False,
            ValueError,
            "dflash adapter uses_non_causal_draft_attention must be true",
        ),
        (
            "injects_target_context_each_layer",
            False,
            ValueError,
            "dflash adapter injects_target_context_each_layer must be true",
        ),
    ),
)
def test_adapter_metadata_requires_dflash_identity_fields(
    field_name, field_value, exc_type, message
):
    kwargs = {
        "algorithm_family": "base_dflash",
        "adapter_version": "test://adapter-version/metadata-required",
        "tokenizer_id": "test://tokenizer/metadata-required",
        "mask_token_id": 0,
        "target_layer_ids": [0, 2],
        "target_feature_schema": "test:hidden_states[batch,seq,hidden]",
        "kv_schema": "test:kv[layer,batch,heads,seq,dim]",
        "target_conditioning_path": "kv_injection_each_draft_layer",
        "draft_output_contract": "block_sequence",
        "max_block_size": 4,
        "uses_non_causal_draft_attention": True,
        "injects_target_context_each_layer": True,
    }
    kwargs[field_name] = field_value
    with pytest.raises(exc_type, match=message):
        DFlashAdapterMetadata(**kwargs)


def test_adapter_metadata_rejects_non_dflash_algorithm_family():
    kwargs = {
        "algorithm_family": "eagle",
        "adapter_version": "test://adapter-version/wrong-family",
        "tokenizer_id": "test://tokenizer/wrong-family",
        "mask_token_id": 0,
        "target_layer_ids": [0, 2],
        "target_feature_schema": "test:hidden_states[batch,seq,hidden]",
        "kv_schema": "test:kv[layer,batch,heads,seq,dim]",
        "target_conditioning_path": "kv_injection_each_draft_layer",
        "draft_output_contract": "block_sequence",
        "max_block_size": 4,
        "uses_non_causal_draft_attention": True,
        "injects_target_context_each_layer": True,
    }
    with pytest.raises(
        ValueError,
        match="dflash adapter algorithm_family must be a DFlash-family value",
    ):
        DFlashAdapterMetadata(**kwargs)


@pytest.mark.parametrize("family", sorted(DFLASH_ALGORITHM_FAMILIES))
def test_adapter_metadata_accepts_declared_dflash_algorithm_families(family):
    draft_output_contract = next(iter(DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS[family]))
    metadata = DFlashAdapterMetadata(
        algorithm_family=family.upper(),
        adapter_version=f"test://adapter-version/{family}",
        tokenizer_id=f"test://tokenizer/{family}",
        mask_token_id=0,
        target_layer_ids=[0, 2],
        target_feature_schema="test:hidden_states[batch,seq,hidden]",
        kv_schema="test:kv[layer,batch,heads,seq,dim]",
        target_conditioning_path="kv_injection_each_draft_layer",
        draft_output_contract=draft_output_contract,
        max_block_size=4,
        uses_non_causal_draft_attention=True,
        injects_target_context_each_layer=True,
    )
    assert metadata.algorithm_family == family


def test_adapter_metadata_accepts_declared_dflash_draft_output_contracts():
    assert set().union(*DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS.values()) == set(
        DFLASH_DRAFT_OUTPUT_CONTRACTS
    )
    family_by_output_contract = {
        next(iter(output_contracts)): family
        for family, output_contracts in DFLASH_ALGORITHM_DRAFT_OUTPUT_CONTRACTS.items()
    }
    for draft_output_contract in sorted(DFLASH_DRAFT_OUTPUT_CONTRACTS):
        family = family_by_output_contract[draft_output_contract]
        metadata = DFlashAdapterMetadata(
            algorithm_family=family,
            adapter_version=f"test://adapter-version/{draft_output_contract}",
            tokenizer_id=f"test://tokenizer/{draft_output_contract}",
            mask_token_id=0,
            target_layer_ids=[0, 2],
            target_feature_schema="test:hidden_states[batch,seq,hidden]",
            kv_schema="test:kv[layer,batch,heads,seq,dim]",
            target_conditioning_path="kv_injection_each_draft_layer",
            draft_output_contract=draft_output_contract.upper(),
            max_block_size=4,
            uses_non_causal_draft_attention=True,
            injects_target_context_each_layer=True,
        )
        assert metadata.draft_output_contract == draft_output_contract
        assert metadata.algorithm_family == family


def test_adapter_metadata_rejects_family_draft_output_contract_mismatch():
    with pytest.raises(
        ValueError,
        match="dflash adapter draft_output_contract is incompatible with algorithm_family",
    ):
        DFlashAdapterMetadata(
            algorithm_family="ddtree",
            adapter_version="test://adapter-version/family-contract-mismatch",
            tokenizer_id="test://tokenizer/family-contract-mismatch",
            mask_token_id=0,
            target_layer_ids=[0, 2],
            target_feature_schema="test:hidden_states[batch,seq,hidden]",
            kv_schema="test:kv[layer,batch,heads,seq,dim]",
            target_conditioning_path="kv_injection_each_draft_layer",
            draft_output_contract="block_sequence",
            max_block_size=4,
            uses_non_causal_draft_attention=True,
            injects_target_context_each_layer=True,
        )


def test_adapter_spec_requires_typed_metadata():
    with pytest.raises(
        TypeError, match="dflash adapter metadata must be DFlashAdapterMetadata"
    ):
        DFlashAdapterSpec(
            name="metadata-type-required",
            target_model_id="test://target/metadata-type-required",
            draft_model_id="test://draft/metadata-type-required",
            provenance="test-only synthetic DFlash adapter fixture",
            metadata={"kind": "loose-dict"},
            supports=lambda _context: True,
            create_runtime=lambda _context: object(),
        )


def test_generic_adapter_returning_non_runtime_raises_typeerror():
    # The sharpest mislabel path: an adapter that registers fine but whose
    # create_runtime returns a *generic* object (e.g. a plain speculative
    # decoder), not a DFlashRuntime. Resolving it under the DFlash name must
    # raise rather than hand back the generic object as if it were DFlash.
    def _supports(context) -> bool:
        return True

    def _create_generic(context):
        # A generic, non-target-conditioned "runtime" — exactly what must NOT be
        # accepted under the DFlash name.
        return {"kind": "generic-speculative-decoder", "draft": lambda *a: [0]}

    spec = _adapter_spec(
        name="generic_mislabel",
        supports=_supports,
        create_runtime=_create_generic,
    )
    register_dflash_adapter(spec)

    with pytest.raises(
        TypeError, match="dflash adapter create_runtime\\(\\) must return DFlashRuntime"
    ):
        resolve_dflash_runtime(
            _ctx("generic_mislabel"), preferred_name="generic_mislabel"
        )


def test_adapter_runtime_block_size_cannot_exceed_checkpoint_metadata():
    def _supports(_context) -> bool:
        return True

    def _create_runtime(_context):
        return DFlashRuntime(
            draft_step=lambda _request: None,
            verify_step=lambda _request: None,
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="block_sequence",
            block_size=8,
        )

    spec = _adapter_spec(
        name="oversized-runtime",
        supports=_supports,
        create_runtime=_create_runtime,
    )
    register_dflash_adapter(spec)

    with pytest.raises(
        ValueError,
        match="dflash runtime block_size exceeds adapter metadata max_block_size",
    ):
        resolve_dflash_runtime(
            _ctx("oversized-runtime"), preferred_name="oversized-runtime"
        )


def test_adapter_runtime_draft_output_contract_must_match_metadata():
    def _supports(_context) -> bool:
        return True

    def _create_runtime(_context):
        return DFlashRuntime(
            draft_step=lambda _request: None,
            verify_step=lambda _request: None,
            initial_conditioning=_valid_conditioning(),
            draft_output_contract="block_sequence",
            block_size=4,
        )

    spec = _adapter_spec(
        name="marginal-metadata",
        supports=_supports,
        create_runtime=_create_runtime,
        metadata=_adapter_metadata(
            "marginal-metadata",
            algorithm_family="ddtree",
            draft_output_contract="per_position_marginals",
        ),
    )
    register_dflash_adapter(spec)

    with pytest.raises(
        ValueError,
        match="dflash runtime draft_output_contract does not match adapter metadata",
    ):
        resolve_dflash_runtime(
            _ctx("marginal-metadata"), preferred_name="marginal-metadata"
        )


def test_selection_context_requires_explicit_dflash_identity():
    with pytest.raises(TypeError, match="target_model_id must be a string"):
        DFlashSelectionContext(
            model=object(),
            backend="native",
            prompt_tokens=[1, 2, 3],
            eos_token_id=None,
            max_new_tokens=8,
            block_size=4,
            target_model_id=None,
            tokenizer_id="test://tokenizer/requires-identity",
        )

    with pytest.raises(ValueError, match="tokenizer_id must be non-empty"):
        DFlashSelectionContext(
            model=object(),
            backend="native",
            prompt_tokens=[1, 2, 3],
            eos_token_id=None,
            max_new_tokens=8,
            block_size=4,
            target_model_id="test://target/requires-identity",
            tokenizer_id=" ",
        )


def test_selection_context_normalizes_token_identity():
    context = DFlashSelectionContext(
        model=object(),
        backend="native",
        prompt_tokens=[1, 2.0, 3],
        eos_token_id=4.0,
        max_new_tokens=8,
        block_size=4,
        target_model_id="test://target/token-identity",
        tokenizer_id="test://tokenizer/token-identity",
    )

    assert context.prompt_tokens == [1, 2, 3]
    assert context.eos_token_id == 4

    with pytest.raises(TypeError, match="prompt_tokens must contain integer token ids"):
        DFlashSelectionContext(
            model=object(),
            backend="native",
            prompt_tokens=[1, True],
            eos_token_id=None,
            max_new_tokens=8,
            block_size=4,
            target_model_id="test://target/token-identity",
            tokenizer_id="test://tokenizer/token-identity",
        )

    with pytest.raises(TypeError, match="eos_token_id must be an integer token id"):
        DFlashSelectionContext(
            model=object(),
            backend="native",
            prompt_tokens=[1, 2, 3],
            eos_token_id=1.5,
            max_new_tokens=8,
            block_size=4,
            target_model_id="test://target/token-identity",
            tokenizer_id="test://tokenizer/token-identity",
        )


def test_adapter_supports_not_called_for_target_tokenizer_mismatch():
    called = False

    def _supports(_context) -> bool:
        nonlocal called
        called = True
        return True

    spec = _adapter_spec(
        name="identity-mismatch",
        supports=_supports,
        create_runtime=lambda _context: object(),
    )
    register_dflash_adapter(spec)

    context = DFlashSelectionContext(
        model=object(),
        backend="native",
        prompt_tokens=[1, 2, 3],
        eos_token_id=None,
        max_new_tokens=8,
        block_size=4,
        target_model_id="test://target/other-model",
        tokenizer_id="test://tokenizer/identity-mismatch",
    )

    assert resolve_dflash_runtime(context, preferred_name="identity-mismatch") is None
    assert called is False


def test_legacy_model_identity_attribute_names_do_not_enable_dflash_resolution():
    class LegacyIdentityModel:
        dflash_target_model_id = "test://target/legacy-identity"
        dflash_tokenizer_id = "test://tokenizer/legacy-identity"
        target_model_id = "test://target/legacy-identity"
        model_id = "test://target/legacy-identity"
        name_or_path = "test://target/legacy-identity"
        tokenizer_id = "test://tokenizer/legacy-identity"

    with pytest.raises(TypeError, match="target_model_id must be a string"):
        DFlashSelectionContext(
            model=LegacyIdentityModel(),
            backend="native",
            prompt_tokens=[1, 2, 3],
            eos_token_id=None,
            max_new_tokens=8,
            block_size=4,
            target_model_id=None,
            tokenizer_id="test://tokenizer/legacy-identity",
        )


def test_named_unavailable_adapter_raises_lookuperror():
    # Requesting a named DFlash adapter that is absent / unsupported must be a
    # typed LookupError — never a silent generic fallback runtime (SPEC.md F6).
    with pytest.raises(
        LookupError,
        match="dflash adapter 'no_such_adapter' is unavailable for this context",
    ):
        build_dflash_runtime(
            model=object(),
            prompt_tokens=[1, 2, 3],
            backend="native",
            dflash_adapter="no_such_adapter",
            target_model_id="test://target/no_such_adapter",
            tokenizer_id="test://tokenizer/no_such_adapter",
        )


def test_unsupported_adapter_under_dflash_name_does_not_resolve():
    # An adapter whose supports() returns False must not resolve under its name;
    # build_dflash_runtime with that name raises LookupError (no fallback).
    def _supports_false(context) -> bool:
        return False

    def _create(context):
        # Would be wrong to ever reach this; present only to prove it is not
        # called when supports() is False.
        raise AssertionError("create_runtime must not run for an unsupported adapter")

    spec = _adapter_spec(
        name="unsupported_adapter",
        supports=_supports_false,
        create_runtime=_create,
    )
    register_dflash_adapter(spec)

    with pytest.raises(
        LookupError,
        match="dflash adapter 'unsupported_adapter' is unavailable for this context",
    ):
        build_dflash_runtime(
            model=object(),
            prompt_tokens=[1, 2, 3],
            backend="native",
            dflash_adapter="unsupported_adapter",
            target_model_id="test://target/unsupported_adapter",
            tokenizer_id="test://tokenizer/unsupported_adapter",
        )


def test_no_adapter_no_name_returns_none_not_generic_runtime():
    # With no preferred name and no registered adapters, build returns None
    # (no DFlash claim) rather than fabricating a generic runtime.
    result = build_dflash_runtime(
        model=object(),
        prompt_tokens=[1, 2, 3],
        backend="native",
        target_model_id="test://target/no-default",
        tokenizer_id="test://tokenizer/no-default",
    )
    assert result is None


# === (c) tinygrad.dflash import must fail closed, pointing at molt.gpu.dflash ===
# Exercises src/molt/stdlib/tinygrad/dflash.py. SPEC.md F6.
#
# A bare `import tinygrad.dflash` under host pytest would resolve `tinygrad` to
# whatever tinygrad is installed in the host environment (the upstream package),
# whose missing `dflash` submodule raises a *generic* ModuleNotFoundError — that
# would test the wrong thing. To exercise MOLT's fail-closed guard file
# directly, load the actual molt stdlib source by path (the same
# spec_from_file_location mechanism the repo's tinygrad_stdlib_loader uses) and
# assert the molt ImportError it raises at module load.

_MOLT_TINYGRAD_DFLASH = (
    Path(__file__).resolve().parents[3]
    / "src"
    / "molt"
    / "stdlib"
    / "tinygrad"
    / "dflash.py"
)


def _load_molt_tinygrad_dflash_guard():
    """Execute molt's tinygrad/dflash.py guard module from source.

    Returns nothing on success path (there is none — the module raises at import
    by design); the caller wraps this in pytest.raises.
    """
    assert _MOLT_TINYGRAD_DFLASH.exists(), (
        f"molt tinygrad.dflash guard missing at {_MOLT_TINYGRAD_DFLASH}"
    )
    spec = importlib.util.spec_from_file_location(
        "molt_stdlib_tinygrad_dflash_probe", _MOLT_TINYGRAD_DFLASH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Executing the module body triggers the module-level raise ImportError.
    spec.loader.exec_module(module)


def test_tinygrad_dflash_guard_raises_importerror():
    with pytest.raises(ImportError) as excinfo:
        _load_molt_tinygrad_dflash_guard()
    message = str(excinfo.value)
    # The fail-closed message must (1) say it's unavailable, (2) explain DFlash
    # needs a target-conditioned block-diffusion drafter/verifier, (3) point at
    # the paper-faithful molt.gpu.dflash contract, and (4) name
    # tinygrad.speculative as the *generic* alternative. Each substring guards a
    # distinct fidelity claim; dropping any is a fidelity regression.
    assert "tinygrad.dflash is not available" in message
    assert "target-conditioned block-diffusion" in message
    assert "molt.gpu.dflash" in message
    assert "tinygrad.speculative" in message
