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

    assert result.tokens == [1, 2, 3]
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

    assert result.tokens == [1, 2, 3]
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

    assert result.tokens == [7, 11]
    assert result.drafted_tokens == 3
    assert result.accepted_draft_tokens == 1
    assert result.target_tokens_emitted == 2
    assert result.verify_calls == 1


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
        timeout=120,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "[1, 2, 3, 4, 5]",
        "4 2",
    ]
