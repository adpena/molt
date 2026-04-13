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
