from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


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
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def test_speculative_decode_greedy_accepts_full_block_and_commits_extra_token():
    from molt.gpu.generate import speculative_decode_greedy

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
    from molt.gpu.generate import speculative_decode_greedy

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
    from molt.gpu.generate import speculative_decode_greedy

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


def test_greedy_decode_routes_through_speculative_engine_when_callbacks_present(monkeypatch):
    import molt.gpu.generate as generate_mod
    from molt.gpu.generate import SpeculativeDecodeResult, greedy_decode

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
            raise AssertionError("default greedy path should not run when speculative callbacks are provided")

    def draft_block(prefix_tokens, block_size):
        return [7][:block_size]

    def verify_block(prefix_tokens, drafted_tokens):
        return [7, 8]

    monkeypatch.setattr(generate_mod, "speculative_decode_greedy", fake_speculative)

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
    from molt.gpu.generate import speculative_decode_greedy

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
        speculative_decode_greedy(verify_short, lambda _p, _n: [1], [0], max_new_tokens=1)
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


def test_speculative_decode_greedy_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_speculative_decode_native.py"
    probe.write_text(
        "from molt.gpu.generate import speculative_decode_greedy\n"
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

    run = subprocess.run(
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
    from molt.gpu.dflash import (
        SpeculativeConditioning,
        SpeculativeDraftRequest,
        SpeculativeDraftResult,
        SpeculativeVerifyRequest,
        SpeculativeVerifyResult,
    )
    from molt.gpu.generate import (
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


def test_conditioned_speculative_decode_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_speculative_conditioned_native.py"
    probe.write_text(
        "from molt.gpu.dflash import (\n"
        "    SpeculativeConditioning,\n"
        "    SpeculativeDraftResult,\n"
        "    SpeculativeVerifyResult,\n"
        ")\n"
        "from molt.gpu.generate import (\n"
        "    speculative_decode_greedy_conditioned,\n"
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

    run = subprocess.run(
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
    import molt.gpu.generate as generate_mod
    from molt.gpu.dflash import (
        DFlashRuntime,
        SpeculativeConditioning,
        SpeculativeDraftResult,
        SpeculativeVerifyResult,
        register_dflash_adapter,
    )
    from molt.gpu.generate import greedy_decode

    class FakeAdapter:
        supported_backends = ("webgpu",)

        def matches(self, model, backend):
            return backend == "webgpu" and getattr(model, "kind", None) == "fake-model"

        def create_runtime(
            self,
            model,
            prompt_tokens,
            *,
            eos_token_id,
            max_new_tokens,
            block_size,
            backend,
        ):
            assert prompt_tokens == [0]
            assert eos_token_id == 11
            assert max_new_tokens == 2
            assert block_size == 4
            assert backend == "webgpu"

            def draft_step(request):
                if request.step_index == 0:
                    return SpeculativeDraftResult([1])
                raise AssertionError(f"unexpected draft step {request.step_index}")

            def verify_step(request):
                return SpeculativeVerifyResult(
                    [1, 2],
                    conditioning=SpeculativeConditioning(target_features="next"),
                )

            return DFlashRuntime(
                draft_step=draft_step,
                verify_step=verify_step,
                initial_conditioning=SpeculativeConditioning(target_features="prefill"),
                block_size=1,
            )

    class FakeModel:
        kind = "fake-model"

        def __call__(self, _tokens):
            raise AssertionError("plain greedy model path should not execute")

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    monkeypatch.setattr(generate_mod, "_DFLASH_DEFAULT_ADAPTER_NAME", "fake-adapter")
    register_dflash_adapter("fake-adapter", FakeAdapter())

    out = greedy_decode(
        FakeModel(),
        [0],
        max_new_tokens=2,
        eos_token_id=11,
        block_size=4,
    )

    assert out == [0, 1, 2]


def test_greedy_decode_skips_dflash_default_without_supported_gpu_backend(monkeypatch):
    from molt.gpu.dflash import register_dflash_adapter
    from molt.gpu.generate import greedy_decode
    from molt.gpu.tensor import Tensor

    calls = []

    class FakeAdapter:
        supported_backends = ("webgpu",)

        def matches(self, model, backend):
            calls.append((getattr(model, "kind", None), backend))
            return True

        def create_runtime(self, *args, **kwargs):
            raise AssertionError("adapter runtime should not be created without supported gpu backend")

    class FakeModel:
        kind = "fake-model"

        def __call__(self, tokens):
            calls.append(("plain", list(tokens)))
            return Tensor([0.0, 1.0, 2.0])

    monkeypatch.delenv("MOLT_GPU_BACKEND", raising=False)
    register_dflash_adapter("fake-adapter-no-gpu", FakeAdapter())

    out = greedy_decode(FakeModel(), [0], max_new_tokens=1)

    assert out == [0, 2]
    assert calls == [("plain", [0])]
