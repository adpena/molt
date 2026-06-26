from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process


@pytest.fixture(autouse=True)
def _isolate_dflash_adapter_registry():
    from molt.gpu.dflash import (
        clear_dflash_adapters,
        restore_dflash_adapters,
        snapshot_dflash_adapters,
    )

    snapshot = snapshot_dflash_adapters()
    clear_dflash_adapters()
    try:
        yield
    finally:
        restore_dflash_adapters(snapshot)


def _native_molt_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_EXT_ROOT"] = str(root)
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    env["TMPDIR"] = str(root / "tmp")
    env["MOLT_STDLIB_PROFILE"] = "full"
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def _dflash_conditioning(tag="prefill", token=0):
    from molt.gpu.dflash import DFlashConditioning

    return DFlashConditioning(
        target_features=f"features-{tag}",
        target_kv=f"kv-{tag}",
        position_ids=[0, 1],
        last_verified_token=token,
        aux={"tag": tag},
    )


_DFLASH_TEST_TARGET_MODEL_ID = "test://target/fake-model"
_DFLASH_TEST_TOKENIZER_ID = "test://tokenizer/fake-model"


def _dflash_identity_kwargs() -> dict[str, str]:
    return {
        "dflash_target_model_id": _DFLASH_TEST_TARGET_MODEL_ID,
        "dflash_tokenizer_id": _DFLASH_TEST_TOKENIZER_ID,
    }


def _dflash_builder_identity_kwargs() -> dict[str, str]:
    return {
        "target_model_id": _DFLASH_TEST_TARGET_MODEL_ID,
        "tokenizer_id": _DFLASH_TEST_TOKENIZER_ID,
    }


def _dflash_adapter_spec(
    *,
    name: str,
    supports,
    create_runtime,
    priority: int = 0,
    target_model_id: str = _DFLASH_TEST_TARGET_MODEL_ID,
    tokenizer_id: str = _DFLASH_TEST_TOKENIZER_ID,
):
    from molt.gpu.dflash import DFlashAdapterMetadata, DFlashAdapterSpec

    return DFlashAdapterSpec(
        name=name,
        target_model_id=target_model_id,
        draft_model_id=f"test://draft/{name}",
        provenance="test-only synthetic DFlash adapter fixture",
        metadata=DFlashAdapterMetadata(
            algorithm_family="base_dflash",
            adapter_version=f"test://adapter-version/{name}",
            tokenizer_id=tokenizer_id,
            mask_token_id=0,
            target_layer_ids=[0, 2],
            target_feature_schema="test:hidden_states[batch,seq,hidden]",
            kv_schema="test:kv[layer,batch,heads,seq,dim]",
            target_conditioning_path="kv_injection_each_draft_layer",
            draft_output_contract="block_sequence",
            max_block_size=4,
            uses_non_causal_draft_attention=True,
            injects_target_context_each_layer=True,
        ),
        supports=supports,
        create_runtime=create_runtime,
        priority=priority,
    )


def test_speculative_decode_greedy_accepts_full_block_and_commits_extra_token():
    from molt.gpu.speculative import speculative_decode_greedy

    def draft_block(prefix_tokens, block_size):
        assert prefix_tokens == [0]
        assert block_size == 2
        return [1, 2]

    def verify_block(prefix_tokens, draft_tokens):
        assert prefix_tokens == [0]
        assert draft_tokens == [1, 2]
        return [1, 2, 3]

    result = speculative_decode_greedy(
        verify_block,
        draft_block,
        [0],
        max_new_tokens=3,
        block_size=2,
    )

    assert result.prompt_tokens == [0]
    assert result.generated_tokens == [1, 2, 3]
    assert result.tokens == [0, 1, 2, 3]
    assert result.drafted_tokens == 2
    assert result.accepted_draft_tokens == 2
    assert result.target_tokens_emitted == 3
    assert result.verify_calls == 1


def test_speculative_decode_greedy_commits_target_token_on_first_mismatch():
    from molt.gpu.speculative import speculative_decode_greedy

    def draft_block(prefix_tokens, block_size):
        if prefix_tokens == [0]:
            assert block_size == 2
            return [1, 9]
        if prefix_tokens == [0, 1, 2]:
            assert block_size == 1
            return [3]
        raise AssertionError(f"unexpected prefix {prefix_tokens!r}")

    def verify_block(prefix_tokens, draft_tokens):
        if prefix_tokens == [0]:
            assert draft_tokens == [1, 9]
            return [1, 2, 7]
        if prefix_tokens == [0, 1, 2]:
            assert draft_tokens == [3]
            return [3, 4]
        raise AssertionError(f"unexpected prefix {prefix_tokens!r}")

    result = speculative_decode_greedy(
        verify_block,
        draft_block,
        [0],
        max_new_tokens=3,
        block_size=2,
    )

    assert result.prompt_tokens == [0]
    assert result.generated_tokens == [1, 2, 3]
    assert result.tokens == [0, 1, 2, 3]
    assert result.drafted_tokens == 3
    assert result.accepted_draft_tokens == 2
    assert result.target_tokens_emitted == 3
    assert result.verify_calls == 2


def test_speculative_decode_greedy_stops_when_target_emits_eos():
    from molt.gpu.speculative import speculative_decode_greedy

    def draft_block(prefix_tokens, block_size):
        assert prefix_tokens == [0]
        assert block_size == 3
        return [7, 8, 9]

    def verify_block(prefix_tokens, draft_tokens):
        assert prefix_tokens == [0]
        assert draft_tokens == [7, 8, 9]
        return [7, 11, 12, 13]

    result = speculative_decode_greedy(
        verify_block,
        draft_block,
        [0],
        max_new_tokens=5,
        block_size=3,
        eos_token_id=11,
    )

    assert result.prompt_tokens == [0]
    assert result.generated_tokens == [7]
    assert result.tokens == [0, 7]
    assert result.drafted_tokens == 3
    assert result.accepted_draft_tokens == 1
    assert result.target_tokens_emitted == 1
    assert result.verify_calls == 1


def test_greedy_decode_routes_through_speculative_engine_when_callbacks_present(
    monkeypatch,
):
    import molt.gpu.generate as generate_mod
    from molt.gpu.generate import greedy_decode
    from molt.gpu.speculative import SpeculativeDecodeResult

    seen = {}

    def fake_speculative(verify_block, draft_block, prompt_tokens, **kwargs):
        seen["verify_block"] = verify_block
        seen["draft_block"] = draft_block
        seen["prompt_tokens"] = list(prompt_tokens)
        seen["kwargs"] = dict(kwargs)
        return SpeculativeDecodeResult(
            [1, 2, 3],
            [4, 5],
            drafted_tokens=2,
            accepted_draft_tokens=2,
            target_tokens_emitted=2,
            verify_calls=1,
        )

    class UnusedModel:
        def __call__(self, _tokens):
            raise AssertionError(
                "default greedy path should not run when speculative callbacks are provided"
            )

    def draft_block(prefix_tokens, block_size):
        return [7][:block_size]

    def verify_block(prefix_tokens, drafted_tokens):
        return [7, 8]

    monkeypatch.setattr(
        generate_mod._speculative, "speculative_decode_greedy", fake_speculative
    )

    out = greedy_decode(
        UnusedModel(),
        [1, 2, 3],
        max_new_tokens=2,
        eos_token_id=11,
        draft_block=draft_block,
        verify_block=verify_block,
        block_size=4,
    )

    assert out == [1, 2, 3, 4, 5]
    assert seen["verify_block"] is verify_block
    assert seen["draft_block"] is draft_block
    assert seen["prompt_tokens"] == [1, 2, 3]
    assert seen["kwargs"] == {
        "max_new_tokens": 2,
        "block_size": 4,
        "eos_token_id": 11,
    }


def test_speculative_decode_greedy_rejects_invalid_callback_contracts():
    from molt.gpu.speculative import speculative_decode_greedy

    def draft_empty(_prefix_tokens, _block_size):
        return []

    def draft_too_large(_prefix_tokens, _block_size):
        return [1, 2, 3]

    def verify_short(_prefix_tokens, _draft_tokens):
        return [1]

    def verify_non_integer(_prefix_tokens, _draft_tokens):
        return [1.5, 2]

    try:
        speculative_decode_greedy(verify_short, draft_empty, [0], max_new_tokens=1)
        raise AssertionError("expected empty draft contract violation")
    except ValueError as exc:
        assert "at least one token" in str(exc)

    try:
        speculative_decode_greedy(verify_short, draft_too_large, [0], max_new_tokens=1)
        raise AssertionError("expected oversized draft contract violation")
    except ValueError as exc:
        assert "more than the requested block size" in str(exc)

    try:
        speculative_decode_greedy(
            verify_short, lambda _p, _n: [1], [0], max_new_tokens=1
        )
        raise AssertionError("expected verify length contract violation")
    except ValueError as exc:
        assert "len(drafted_tokens) + 1" in str(exc)

    try:
        speculative_decode_greedy(
            verify_non_integer,
            lambda _p, _n: [1],
            [0],
            max_new_tokens=1,
        )
        raise AssertionError("expected integer token contract violation")
    except TypeError as exc:
        assert "integer token ids" in str(exc)


def test_speculative_decode_bounds_contract_is_shared_across_transports():
    from molt.gpu.speculative import (
        SpeculativeConditioning,
        SpeculativeDraftResult,
        SpeculativeVerifyResult,
        speculative_decode_greedy,
        speculative_decode_greedy_conditioned,
    )

    def draft_block(_prefix_tokens, _block_size):
        return [1]

    def verify_block(_prefix_tokens, _draft_tokens):
        return [1, 2]

    def draft_step(_request):
        return SpeculativeDraftResult([1])

    def verify_step(_request):
        return SpeculativeVerifyResult([1, 2])

    with pytest.raises(
        TypeError, match="max_new_tokens must be a non-negative integer"
    ):
        speculative_decode_greedy(
            verify_block,
            draft_block,
            [0],
            max_new_tokens=True,
        )

    with pytest.raises(ValueError, match="max_new_tokens must be non-negative"):
        speculative_decode_greedy_conditioned(
            verify_step,
            draft_step,
            [0],
            initial_conditioning=SpeculativeConditioning(),
            max_new_tokens=-1,
        )

    with pytest.raises(TypeError, match="block_size must be a positive integer"):
        speculative_decode_greedy_conditioned(
            verify_step,
            draft_step,
            [0],
            initial_conditioning=SpeculativeConditioning(),
            block_size=True,
        )

    with pytest.raises(ValueError, match="block_size must be positive"):
        speculative_decode_greedy(
            verify_block,
            draft_block,
            [0],
            block_size=0,
        )

    with pytest.raises(TypeError, match="eos_token_id must be an integer token id"):
        speculative_decode_greedy(
            verify_block,
            draft_block,
            [0],
            eos_token_id=True,
        )

    with pytest.raises(TypeError, match="eos_token_id must be an integer token id"):
        speculative_decode_greedy_conditioned(
            verify_step,
            draft_step,
            [0],
            initial_conditioning=SpeculativeConditioning(),
            eos_token_id=1.5,
        )


def test_speculative_decode_greedy_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_speculative_decode_native.py"
    probe.write_text(
        "from molt.gpu.speculative import speculative_decode_greedy\n"
        "\n"
        "def draft_block(prefix_tokens, block_size):\n"
        "    if prefix_tokens == [0]:\n"
        "        return [1, 2][:block_size]\n"
        "    return [4, 5][:block_size]\n"
        "\n"
        "def verify_block(prefix_tokens, draft_tokens):\n"
        "    if prefix_tokens == [0]:\n"
        "        return [1, 2, 3]\n"
        "    return [4, 5, 6]\n"
        "\n"
        "result = speculative_decode_greedy(\n"
        "    verify_block,\n"
        "    draft_block,\n"
        "    [0],\n"
        "    max_new_tokens=5,\n"
        "    block_size=2,\n"
        ")\n"
        "print(result.tokens)\n"
        "print(result.generated_tokens)\n"
        "print(result.accepted_draft_tokens, result.verify_calls)\n",
        encoding="utf-8",
    )

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "[0, 1, 2, 3, 4, 5]",
        "[1, 2, 3, 4, 5]",
        "4 2",
    ]


def test_conditioned_speculative_decode_propagates_target_conditioning():
    from molt.gpu.speculative import (
        SpeculativeConditioning,
        SpeculativeDraftRequest,
        SpeculativeDraftResult,
        SpeculativeVerifyRequest,
        SpeculativeVerifyResult,
    )
    from molt.gpu.speculative import (
        speculative_decode_greedy_conditioned,
    )

    draft_seen = []
    verify_seen = []

    def draft_step(request):
        assert isinstance(request, SpeculativeDraftRequest)
        draft_seen.append(
            (
                list(request.prefix_tokens),
                request.max_block_size,
                request.step_index,
                request.conditioning.target_features,
                request.conditioning.target_kv,
            )
        )
        if request.step_index == 0:
            return SpeculativeDraftResult([1, 2])
        return SpeculativeDraftResult([4])

    def verify_step(request):
        assert isinstance(request, SpeculativeVerifyRequest)
        verify_seen.append(
            (
                list(request.prefix_tokens),
                list(request.draft_tokens),
                request.conditioning.target_features,
                request.conditioning.target_kv,
            )
        )
        if request.prefix_tokens == [0]:
            return SpeculativeVerifyResult(
                [1, 2, 3],
                conditioning=SpeculativeConditioning(
                    target_features="verify-1",
                    target_kv="kv-1",
                ),
            )
        return SpeculativeVerifyResult(
            [4, 5],
            conditioning=SpeculativeConditioning(
                target_features="verify-2",
                target_kv="kv-2",
            ),
        )

    result = speculative_decode_greedy_conditioned(
        verify_step,
        draft_step,
        [0],
        initial_conditioning=SpeculativeConditioning(
            target_features="prefill",
            target_kv="prefill-kv",
        ),
        max_new_tokens=4,
        block_size=2,
    )

    assert result.tokens == [0, 1, 2, 3, 4]
    assert draft_seen == [
        ([0], 2, 0, "prefill", "prefill-kv"),
        ([0, 1, 2, 3], 1, 1, "verify-1", "kv-1"),
    ]
    assert verify_seen == [
        ([0], [1, 2], "prefill", "prefill-kv"),
        ([0, 1, 2, 3], [4], "verify-1", "kv-1"),
    ]


def test_conditioned_speculative_decode_rejects_loose_transport_contracts():
    from molt.gpu.speculative import (
        SpeculativeConditioning,
        SpeculativeDraftResult,
        SpeculativeVerifyResult,
        speculative_decode_greedy_conditioned,
    )

    def valid_draft(_request):
        return SpeculativeDraftResult([1])

    def valid_verify(_request):
        return SpeculativeVerifyResult([1, 2])

    try:
        speculative_decode_greedy_conditioned(
            valid_verify,
            valid_draft,
            [0],
            initial_conditioning=object(),
            max_new_tokens=1,
            block_size=1,
        )
        raise AssertionError("expected initial conditioning type failure")
    except TypeError as exc:
        assert "initial_conditioning must be SpeculativeConditioning" in str(exc)

    try:
        speculative_decode_greedy_conditioned(
            valid_verify,
            lambda _request: [1],
            [0],
            initial_conditioning=SpeculativeConditioning(),
            max_new_tokens=1,
            block_size=1,
        )
        raise AssertionError("expected draft result type failure")
    except TypeError as exc:
        assert "draft_step must return SpeculativeDraftResult" in str(exc)

    try:
        speculative_decode_greedy_conditioned(
            verify_step=lambda _request: [1, 2],
            draft_step=valid_draft,
            prompt_tokens=[0],
            initial_conditioning=SpeculativeConditioning(),
            max_new_tokens=1,
            block_size=1,
        )
        raise AssertionError("expected verify result type failure")
    except TypeError as exc:
        assert "verify_step must return SpeculativeVerifyResult" in str(exc)

    def verify_with_loose_refresh(_request):
        return SpeculativeVerifyResult([1, 2], conditioning=object())

    try:
        speculative_decode_greedy_conditioned(
            verify_with_loose_refresh,
            valid_draft,
            [0],
            initial_conditioning=SpeculativeConditioning(),
            max_new_tokens=1,
            block_size=1,
        )
        raise AssertionError("expected refreshed conditioning type failure")
    except TypeError as exc:
        assert "verify_result.conditioning must be SpeculativeConditioning" in str(exc)


def test_dflash_conditioning_requires_target_payloads():
    from molt.gpu.dflash import DFlashConditioning

    try:
        DFlashConditioning(
            target_features=None,
            target_kv="kv",
            position_ids=[0],
            last_verified_token=0,
        )
        raise AssertionError("expected missing target_features failure")
    except ValueError as exc:
        assert "target_features" in str(exc)

    try:
        DFlashConditioning(
            target_features="features",
            target_kv=None,
            position_ids=[0],
            last_verified_token=0,
        )
        raise AssertionError("expected missing target_kv failure")
    except ValueError as exc:
        assert "target_kv" in str(exc)

    try:
        DFlashConditioning(
            target_features="features",
            target_kv="kv",
            position_ids=None,
            last_verified_token=0,
        )
        raise AssertionError("expected missing position_ids failure")
    except ValueError as exc:
        assert "position_ids" in str(exc)


def test_dflash_runtime_requires_target_conditioned_initial_payload():
    from molt.gpu.dflash import DFlashRuntime
    from molt.gpu.speculative import SpeculativeConditioning

    try:
        DFlashRuntime(
            draft_step=lambda _request: None,
            verify_step=lambda _request: None,
            initial_conditioning=SpeculativeConditioning(
                target_features="features",
                target_kv="kv",
                position_ids=[0],
            ),
            draft_output_contract="block_sequence",
        )
        raise AssertionError("expected DFlashConditioning failure")
    except TypeError as exc:
        assert "DFlashConditioning" in str(exc)


def test_conditioned_decode_rejects_invalid_dflash_conditioning_refresh():
    from molt.gpu.speculative import (
        SpeculativeConditioning,
        SpeculativeDraftResult,
        SpeculativeVerifyResult,
    )
    from molt.gpu.speculative import speculative_decode_greedy_conditioned

    def draft_step(_request):
        return SpeculativeDraftResult([1])

    def verify_step(_request):
        return SpeculativeVerifyResult(
            [1, 2],
            conditioning=SpeculativeConditioning(
                target_features="features",
                target_kv="kv",
                position_ids=[0],
            ),
        )

    try:
        speculative_decode_greedy_conditioned(
            verify_step,
            draft_step,
            [0],
            initial_conditioning=_dflash_conditioning("prefill", token=0),
            max_new_tokens=1,
            block_size=1,
        )
        raise AssertionError("expected invalid refreshed DFlash conditioning failure")
    except TypeError as exc:
        assert "DFlashConditioning" in str(exc)


def test_tinygrad_dflash_import_fails_closed_with_provenance(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_dflash_import_probe.py"
    probe.write_text(
        "import importlib.util\n"
        "try:\n"
        f"    spec = importlib.util.spec_from_file_location('tinygrad.dflash', {str(root / 'src' / 'molt' / 'stdlib' / 'tinygrad' / 'dflash.py')!r})\n"
        "    module = importlib.util.module_from_spec(spec)\n"
        "    spec.loader.exec_module(module)\n"
        "    raise AssertionError('tinygrad.dflash import should fail closed')\n"
        "except ImportError as exc:\n"
        "    print(str(exc))\n",
        encoding="utf-8",
    )
    run = run_native_test_process(
        [sys.executable, str(probe)],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert "target-conditioned block-diffusion" in run.stdout
    assert "molt.gpu.dflash" in run.stdout
    assert "tinygrad.speculative" in run.stdout


def test_conditioned_speculative_decode_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_speculative_conditioned_native.py"
    probe.write_text(
        "from molt.gpu.speculative import speculative_decode_greedy_conditioned\n"
        "from molt.gpu.speculative import (\n"
        "    SpeculativeConditioning,\n"
        "    SpeculativeDraftResult,\n"
        "    SpeculativeVerifyResult,\n"
        ")\n"
        "\n"
        "def draft_step(request):\n"
        "    if request.step_index == 0:\n"
        "        return SpeculativeDraftResult([1, 2])\n"
        "    return SpeculativeDraftResult([4])\n"
        "\n"
        "def verify_step(request):\n"
        "    if request.prefix_tokens == [0]:\n"
        "        return SpeculativeVerifyResult(\n"
        "            [1, 2, 3],\n"
        "            conditioning=SpeculativeConditioning(target_features=10, target_kv=20),\n"
        "        )\n"
        "    return SpeculativeVerifyResult(\n"
        "        [4, 5],\n"
        "        conditioning=SpeculativeConditioning(target_features=30, target_kv=40),\n"
        "    )\n"
        "\n"
        "result = speculative_decode_greedy_conditioned(\n"
        "    verify_step,\n"
        "    draft_step,\n"
        "    [0],\n"
        "    initial_conditioning=SpeculativeConditioning(target_features=1, target_kv=2),\n"
        "    max_new_tokens=4,\n"
        "    block_size=2,\n"
        ")\n"
        "print(result.tokens)\n"
        "print(result.generated_tokens)\n",
        encoding="utf-8",
    )

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "[0, 1, 2, 3, 4]",
        "[1, 2, 3, 4]",
    ]


def test_greedy_decode_uses_registered_dflash_adapter_by_default_on_gpu_backend(
    monkeypatch,
):
    from molt.gpu.dflash import (
        DFlashRuntime,
        register_dflash_adapter,
    )
    from molt.gpu.generate import greedy_decode
    from molt.gpu.speculative import SpeculativeDraftResult, SpeculativeVerifyResult

    def supports(context):
        return (
            context.backend == "webgpu"
            and getattr(context.model, "kind", None) == "fake-model"
        )

    def create_runtime(context):
        assert context.prompt_tokens == [0]
        assert context.eos_token_id == 11
        assert context.max_new_tokens == 2
        assert context.block_size == 4
        assert context.backend == "webgpu"

        def draft_step(request):
            if request.step_index == 0:
                return SpeculativeDraftResult([1])
            raise AssertionError(f"unexpected draft step {request.step_index}")

        def verify_step(request):
            return SpeculativeVerifyResult(
                [1, 2],
                conditioning=_dflash_conditioning("next", token=1),
            )

        return DFlashRuntime(
            draft_step=draft_step,
            verify_step=verify_step,
            initial_conditioning=_dflash_conditioning("prefill", token=0),
            draft_output_contract="block_sequence",
            block_size=2,
        )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", " WEBGPU ")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="fake-adapter",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=2,
        eos_token_id=11,
        block_size=4,
        **_dflash_identity_kwargs(),
    )

    assert out == [0, 1, 2]


def test_greedy_decode_dflash_adapter_refreshes_target_conditioning_after_rejection(
    monkeypatch,
):
    from molt.gpu.dflash import (
        DFlashConditioning,
        DFlashRuntime,
        register_dflash_adapter,
    )
    from molt.gpu.generate import greedy_decode
    from molt.gpu.speculative import SpeculativeDraftResult, SpeculativeVerifyResult

    events = []

    def conditioning_payload(conditioning):
        return {
            "target_features": conditioning.target_features,
            "target_kv": conditioning.target_kv,
            "position_ids": conditioning.position_ids,
            "last_verified_token": conditioning.last_verified_token,
            "tag": conditioning.aux["tag"],
        }

    def assert_conditioning_payload(conditioning, tag, token):
        assert isinstance(conditioning, DFlashConditioning)
        assert conditioning_payload(conditioning) == {
            "target_features": f"features-{tag}",
            "target_kv": f"kv-{tag}",
            "position_ids": [0, 1],
            "last_verified_token": token,
            "tag": tag,
        }

    def supports(context):
        return (
            context.backend == "webgpu"
            and getattr(context.model, "kind", None) == "fake-model"
        )

    def create_runtime(context):
        assert context.prompt_tokens == [0]
        assert context.backend == "webgpu"
        assert context.block_size == 2
        assert context.max_new_tokens == 4

        initial_conditioning = _dflash_conditioning("prefill", token=0)

        def draft_step(request):
            assert not hasattr(request, "draft_tokens")
            assert hasattr(request, "max_block_size")
            if request.step_index == 0:
                assert request.prefix_tokens == [0]
                assert request.max_block_size == 2
                assert_conditioning_payload(request.conditioning, "prefill", 0)
                events.append(
                    (
                        "draft",
                        request.step_index,
                        list(request.prefix_tokens),
                        request.max_block_size,
                        conditioning_payload(request.conditioning),
                    )
                )
                return SpeculativeDraftResult([1, 9])
            if request.step_index == 1:
                assert request.prefix_tokens == [0, 1, 2]
                assert request.max_block_size == 2
                assert_conditioning_payload(request.conditioning, "refresh", 2)
                events.append(
                    (
                        "draft",
                        request.step_index,
                        list(request.prefix_tokens),
                        request.max_block_size,
                        conditioning_payload(request.conditioning),
                    )
                )
                return SpeculativeDraftResult([3])
            raise AssertionError(f"unexpected draft step {request.step_index}")

        def verify_step(request):
            assert hasattr(request, "draft_tokens")
            assert not hasattr(request, "max_block_size")
            if request.step_index == 0:
                assert request.prefix_tokens == [0]
                assert request.draft_tokens == [1, 9]
                assert_conditioning_payload(request.conditioning, "prefill", 0)
                events.append(
                    (
                        "verify",
                        request.step_index,
                        list(request.prefix_tokens),
                        list(request.draft_tokens),
                        conditioning_payload(request.conditioning),
                    )
                )
                return SpeculativeVerifyResult(
                    [1, 2, 7],
                    conditioning=_dflash_conditioning("refresh", token=2),
                )
            if request.step_index == 1:
                assert request.prefix_tokens == [0, 1, 2]
                assert request.draft_tokens == [3]
                assert_conditioning_payload(request.conditioning, "refresh", 2)
                events.append(
                    (
                        "verify",
                        request.step_index,
                        list(request.prefix_tokens),
                        list(request.draft_tokens),
                        conditioning_payload(request.conditioning),
                    )
                )
                return SpeculativeVerifyResult(
                    [3, 4],
                    conditioning=_dflash_conditioning("final", token=3),
                )
            raise AssertionError(f"unexpected verify step {request.step_index}")

        return DFlashRuntime(
            draft_step=draft_step,
            verify_step=verify_step,
            initial_conditioning=initial_conditioning,
            draft_output_contract="block_sequence",
            block_size=2,
        )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="fake-refreshing-adapter",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=4,
        block_size=2,
        **_dflash_identity_kwargs(),
    )

    assert out == [0, 1, 2, 3, 4]
    assert [(event[0], event[1], event[-1]["tag"]) for event in events] == [
        ("draft", 0, "prefill"),
        ("verify", 0, "prefill"),
        ("draft", 1, "refresh"),
        ("verify", 1, "refresh"),
    ]


def test_greedy_decode_fails_closed_when_registered_dflash_adapter_has_no_trained_drafter(
    monkeypatch,
):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode

    def create_runtime(_context):
        raise AssertionError("unsupported trained drafter must not be created")

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="trained-drafter-only",
            supports=lambda _context: False,
            create_runtime=create_runtime,
        )
    )

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="trained-drafter-only",
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected unavailable trained drafter failure")
    except LookupError as exc:
        assert "trained-drafter-only" in str(exc)


def test_greedy_decode_chooses_highest_priority_matching_dflash_adapter(monkeypatch):
    from molt.gpu.dflash import (
        DFlashRuntime,
        register_dflash_adapter,
    )
    from molt.gpu.generate import greedy_decode
    from molt.gpu.speculative import SpeculativeDraftResult, SpeculativeVerifyResult

    def low_supports(context):
        return context.backend == "webgpu"

    def high_supports(context):
        return context.backend == "webgpu"

    def low_create_runtime(_context):
        raise AssertionError("lower-priority adapter should not win")

    def high_create_runtime(_context):
        def draft_step(request):
            return SpeculativeDraftResult([1])

        def verify_step(request):
            return SpeculativeVerifyResult([1, 2])

        return DFlashRuntime(
            draft_step=draft_step,
            verify_step=verify_step,
            block_size=2,
            initial_conditioning=_dflash_conditioning("priority", token=0),
            draft_output_contract="block_sequence",
        )

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="priority-low-adapter",
            supports=low_supports,
            create_runtime=low_create_runtime,
            priority=1,
        )
    )
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="priority-high-adapter",
            supports=high_supports,
            create_runtime=high_create_runtime,
            priority=2,
        )
    )

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=2,
        block_size=4,
        **_dflash_identity_kwargs(),
    )

    assert out == [0, 1, 2]


def test_greedy_decode_raises_when_top_priority_dflash_adapters_are_ambiguous(
    monkeypatch,
):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode

    def supports(context):
        return context.backend == "webgpu"

    def create_runtime(_context):
        raise AssertionError("ambiguous adapters must raise before runtime creation")

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="ambiguous-a",
            supports=supports,
            create_runtime=create_runtime,
            priority=5,
        )
    )
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="ambiguous-b",
            supports=supports,
            create_runtime=create_runtime,
            priority=5,
        )
    )

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected ambiguous dflash adapter failure")
    except ValueError as exc:
        assert "multiple dflash adapters match" in str(exc)
        assert "ambiguous-a" in str(exc)
        assert "ambiguous-b" in str(exc)


def test_greedy_decode_skips_dflash_default_without_supported_gpu_backend(monkeypatch):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode
    from molt.gpu.tensor import Tensor

    calls = []

    def supports(context):
        calls.append((getattr(context.model, "kind", None), context.backend))
        return True

    def create_runtime(_context):
        raise AssertionError(
            "adapter runtime should not be created without supported gpu backend"
        )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, tokens):
            calls.append(("plain", list(tokens)))
            return Tensor([0.0, 1.0, 2.0])

    monkeypatch.delenv("MOLT_GPU_BACKEND", raising=False)
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="fake-adapter-no-gpu",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    out = greedy_decode(FakeModel(), [0], max_new_tokens=1)

    assert out == [0, 2]
    assert calls == [("plain", [0])]


def test_greedy_decode_skips_dflash_default_on_unsupported_backend(monkeypatch):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode
    from molt.gpu.tensor import Tensor

    calls = []

    def supports(context):
        calls.append((getattr(context.model, "kind", None), context.backend))
        return True

    def create_runtime(_context):
        raise AssertionError(
            "adapter runtime should not be created for unsupported backend"
        )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, tokens):
            calls.append(("plain", list(tokens)))
            return Tensor([0.0, 1.0, 2.0])

    monkeypatch.setenv("MOLT_GPU_BACKEND", "native")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="fake-adapter-unsupported-backend",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=1,
        **_dflash_identity_kwargs(),
    )

    assert out == [0, 2]
    assert calls == [("plain", [0])]


def test_greedy_decode_raises_when_gpu_backend_has_no_dflash_adapter(monkeypatch):
    from molt.gpu.dflash import clear_dflash_adapters
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    clear_dflash_adapters()

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected missing dflash adapter failure")
    except LookupError as exc:
        assert "no dflash adapter is available" in str(exc)


def test_greedy_decode_allows_plain_decode_when_dflash_preference_disabled(monkeypatch):
    from molt.gpu.generate import greedy_decode
    from molt.gpu.tensor import Tensor

    class FakeModel:
        def __call__(self, tokens):
            return Tensor([0.0, 1.0, 2.0 + len(tokens)])

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")

    out = greedy_decode(FakeModel(), [0], max_new_tokens=1, prefer_dflash=False)

    assert out == [0, 2]


def test_register_dflash_adapter_rejects_duplicate_names():
    from molt.gpu.dflash import register_dflash_adapter

    spec = _dflash_adapter_spec(
        name="duplicate-test-adapter",
        supports=lambda _context: True,
        create_runtime=lambda _context: None,
    )
    register_dflash_adapter(spec)

    try:
        register_dflash_adapter(spec)
        raise AssertionError("expected duplicate adapter registration failure")
    except ValueError as exc:
        assert "already registered" in str(exc)


def test_dflash_adapter_registry_snapshot_restore_isolates_mutation():
    from molt.gpu.dflash import (
        clear_dflash_adapters,
        get_dflash_adapter,
        list_dflash_adapters,
        register_dflash_adapter,
        restore_dflash_adapters,
        snapshot_dflash_adapters,
    )

    baseline = snapshot_dflash_adapters()
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="snapshot-adapter",
            supports=lambda _context: True,
            create_runtime=lambda _context: None,
        )
    )
    mutated = snapshot_dflash_adapters()

    clear_dflash_adapters()
    assert list_dflash_adapters() == []

    restore_dflash_adapters(mutated)
    assert get_dflash_adapter("snapshot-adapter") is not None

    restore_dflash_adapters(baseline)
    assert get_dflash_adapter("snapshot-adapter") is None


def test_greedy_decode_accepts_explicit_dflash_adapter_override(monkeypatch):
    from molt.gpu.dflash import (
        DFlashRuntime,
        register_dflash_adapter,
    )
    from molt.gpu.generate import greedy_decode
    from molt.gpu.speculative import SpeculativeDraftResult, SpeculativeVerifyResult

    def supports(context):
        return context.backend == "webgpu"

    def create_runtime(_context):
        def draft_step(request):
            return SpeculativeDraftResult([1])

        def verify_step(request):
            return SpeculativeVerifyResult(
                [1, 2],
                conditioning=_dflash_conditioning("next", token=1),
            )

        return DFlashRuntime(
            draft_step=draft_step,
            verify_step=verify_step,
            initial_conditioning=_dflash_conditioning("prefill", token=0),
            draft_output_contract="block_sequence",
            block_size=2,
        )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="explicit-adapter",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=2,
        eos_token_id=11,
        dflash_adapter="explicit-adapter",
        block_size=4,
        **_dflash_identity_kwargs(),
    )

    assert out == [0, 1, 2]


def test_greedy_decode_raises_for_missing_explicit_dflash_adapter(monkeypatch):
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="missing-drafter",
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected missing dflash adapter failure")
    except LookupError as exc:
        assert "missing-drafter" in str(exc)


def test_greedy_decode_raises_for_explicit_dflash_adapter_without_backend(monkeypatch):
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.delenv("MOLT_GPU_BACKEND", raising=False)

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="requires-backend",
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected explicit DFlash request to fail closed")
    except LookupError as exc:
        assert "requires-backend" in str(exc)
        assert "backend" in str(exc).lower()


def test_greedy_decode_raises_for_explicit_dflash_adapter_on_unsupported_backend(
    monkeypatch,
):
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "native")

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="requires-webgpu-or-metal",
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected explicit DFlash request to fail closed")
    except LookupError as exc:
        assert "requires-webgpu-or-metal" in str(exc)
        assert "DFlash-capable GPU backend" in str(exc)
        assert "webgpu" in str(exc)
        assert "metal" in str(exc)


def test_greedy_decode_requires_explicit_dflash_identity_on_gpu_backend(monkeypatch):
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")

    try:
        greedy_decode(FakeModel(), [0], max_new_tokens=1)
        raise AssertionError("expected missing explicit DFlash identity failure")
    except TypeError as exc:
        assert "target_model_id" in str(exc)


def test_greedy_decode_ignores_model_declared_dflash_adapter(monkeypatch):
    from molt.gpu.generate import greedy_decode

    class FakeModel:
        dflash_adapter = "missing-model-drafter"
        dflash_target_model_id = _DFLASH_TEST_TARGET_MODEL_ID
        dflash_tokenizer_id = _DFLASH_TEST_TOKENIZER_ID

        def __call__(self, _tokens):
            raise AssertionError("plain greedy fallback must not run")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")

    try:
        greedy_decode(FakeModel(), [0], max_new_tokens=1, **_dflash_identity_kwargs())
        raise AssertionError("expected missing dflash adapter failure")
    except LookupError as exc:
        assert "no dflash adapter is available" in str(exc)
        assert "missing-model-drafter" not in str(exc)


def test_greedy_decode_rejects_non_boolean_dflash_supports_result(monkeypatch):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode

    def supports(_context):
        return "yes"

    def create_runtime(_context):
        raise AssertionError(
            "invalid supports result should fail before runtime creation"
        )

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="bad-supports",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="bad-supports",
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected boolean supports contract failure")
    except TypeError as exc:
        assert "supports" in str(exc)
        assert "bool" in str(exc)


def test_greedy_decode_rejects_invalid_dflash_runtime_type(monkeypatch):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode

    def supports(_context):
        return True

    def create_runtime(_context):
        return object()

    class FakeModel:
        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    register_dflash_adapter(
        _dflash_adapter_spec(
            name="bad-runtime",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    try:
        greedy_decode(
            FakeModel(),
            [0],
            max_new_tokens=1,
            dflash_adapter="bad-runtime",
            block_size=4,
            **_dflash_identity_kwargs(),
        )
        raise AssertionError("expected invalid runtime type failure")
    except TypeError as exc:
        assert "DFlashRuntime" in str(exc)


def test_dflash_namespace_does_not_export_generic_speculative_helpers():
    import molt.gpu.dflash as dflash

    generic_names = (
        "SpeculativeConditioning",
        "SpeculativeDraftRequest",
        "SpeculativeDraftResult",
        "SpeculativeVerifyRequest",
        "SpeculativeVerifyResult",
        "speculative_decode_greedy",
        "speculative_decode_greedy_conditioned",
    )
    for name in generic_names:
        assert name not in dflash.__all__
        assert not hasattr(dflash, name)


def test_generate_namespace_does_not_export_generic_speculative_helpers():
    import molt.gpu.generate as generate

    generic_names = (
        "SpeculativeDecodeResult",
        "speculative_decode_greedy",
        "speculative_decode_greedy_conditioned",
    )
    for name in generic_names:
        assert name not in generate.__all__
        assert not hasattr(generate, name)


def test_build_dflash_runtime_constructs_runtime_from_explicit_adapter():
    from molt.gpu.dflash import (
        DFlashRuntime,
        build_dflash_runtime,
        register_dflash_adapter,
    )

    def supports(context):
        return context.backend == "webgpu"

    def create_runtime(context):
        assert context.prompt_tokens == [0]
        assert context.backend == "webgpu"
        return DFlashRuntime(
            draft_step=lambda _request: None,
            verify_step=lambda _request: None,
            block_size=4,
            initial_conditioning=_dflash_conditioning("builder", token=0),
            draft_output_contract="block_sequence",
        )

    class FakeModel:
        pass

    register_dflash_adapter(
        _dflash_adapter_spec(
            name="builder-adapter",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    runtime = build_dflash_runtime(
        FakeModel(),
        [0],
        backend="webgpu",
        dflash_adapter="builder-adapter",
        max_new_tokens=8,
        block_size=4,
        **_dflash_builder_identity_kwargs(),
    )

    assert isinstance(runtime, DFlashRuntime)
    assert runtime.block_size == 4


def test_build_dflash_runtime_raises_for_missing_explicit_adapter():
    from molt.gpu.dflash import build_dflash_runtime

    class FakeModel:
        pass

    try:
        build_dflash_runtime(
            FakeModel(),
            [0],
            backend="webgpu",
            dflash_adapter="missing-adapter",
            block_size=4,
            **_dflash_builder_identity_kwargs(),
        )
        raise AssertionError("expected missing explicit adapter failure")
    except LookupError as exc:
        assert "missing-adapter" in str(exc)


def test_build_dflash_runtime_raises_for_explicit_adapter_on_unsupported_backend():
    from molt.gpu.dflash import build_dflash_runtime

    class FakeModel:
        pass

    try:
        build_dflash_runtime(
            FakeModel(),
            [0],
            backend="native",
            dflash_adapter="requires-webgpu-or-metal",
            block_size=4,
            **_dflash_builder_identity_kwargs(),
        )
        raise AssertionError("expected unsupported backend failure")
    except LookupError as exc:
        assert "requires-webgpu-or-metal" in str(exc)
        assert "DFlash-capable GPU backend" in str(exc)
        assert "webgpu" in str(exc)
        assert "metal" in str(exc)


def test_build_dflash_runtime_passes_adapter_payload_into_context():
    from molt.gpu.dflash import (
        DFlashRuntime,
        build_dflash_runtime,
        register_dflash_adapter,
    )

    seen = {}

    def supports(context):
        seen["payload"] = context.adapter_payload
        return context.adapter_payload == {"patch_features": "patches"}

    def create_runtime(context):
        seen["runtime_payload"] = context.adapter_payload
        return DFlashRuntime(
            draft_step=lambda _request: None,
            verify_step=lambda _request: None,
            block_size=2,
            initial_conditioning=_dflash_conditioning("payload", token=0),
            draft_output_contract="block_sequence",
        )

    class FakeModel:
        pass

    register_dflash_adapter(
        _dflash_adapter_spec(
            name="payload-adapter",
            supports=supports,
            create_runtime=create_runtime,
        )
    )

    runtime = build_dflash_runtime(
        FakeModel(),
        [0],
        backend="webgpu",
        dflash_adapter="payload-adapter",
        block_size=4,
        adapter_payload={"patch_features": "patches"},
        **_dflash_builder_identity_kwargs(),
    )

    assert isinstance(runtime, DFlashRuntime)
    assert seen == {
        "payload": {"patch_features": "patches"},
        "runtime_payload": {"patch_features": "patches"},
    }


def test_speculative_conditioning_carries_multimodal_payload_fields():
    from molt.gpu.speculative import SpeculativeConditioning

    conditioning = SpeculativeConditioning(
        target_features={"hidden": [1, 2]},
        target_kv={"layer0": "kv"},
        patch_features="patches",
        position_ids=[0, 1, 1, 2],
        aux={"source": "falcon"},
    )

    assert conditioning.target_features == {"hidden": [1, 2]}
    assert conditioning.target_kv == {"layer0": "kv"}
    assert conditioning.patch_features == "patches"
    assert conditioning.position_ids == [0, 1, 1, 2]
    assert conditioning.aux == {"source": "falcon"}
