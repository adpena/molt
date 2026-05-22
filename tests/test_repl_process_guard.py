from __future__ import annotations

import builtins
import subprocess
from pathlib import Path
from typing import Any

import pytest

from molt import repl


def test_run_repl_uses_memory_guard_and_canonical_tmp(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    inputs = iter(["1 + 1", "exit()"])
    calls: list[dict[str, Any]] = []
    snippet_paths: list[Path] = []

    monkeypatch.setattr(builtins, "input", lambda _prompt="": next(inputs))

    def fake_timeout_from_env(
        prefix: str,
        env: dict[str, str],
        *,
        explicit: float | None = None,
        default: float | None = None,
        cwd: Path | None = None,
        **_kwargs: object,
    ) -> float:
        calls.append(
            {
                "method": "timeout",
                "prefix": prefix,
                "env": env,
                "explicit": explicit,
                "default": default,
                "cwd": cwd,
            }
        )
        return 12.5

    def fake_run_completed_command(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        snippet_path = Path(cmd[2])
        snippet_paths.append(snippet_path)
        calls.append({"method": "run", "cmd": cmd, **kwargs})
        assert snippet_path.read_text(encoding="utf-8") == "print(repr(1 + 1))"
        return subprocess.CompletedProcess(cmd, 0, "2\n", "")

    monkeypatch.setattr(
        repl.process_guard,
        "timeout_from_env",
        fake_timeout_from_env,
        raising=True,
    )
    monkeypatch.setattr(
        repl.process_guard,
        "run_completed_command",
        fake_run_completed_command,
        raising=True,
    )

    rc = repl.run_repl(
        capabilities="fs.read",
        io_mode="virtual",
        molt_cmd=["molt-dev"],
    )

    assert rc == 0
    assert "2\n" in capsys.readouterr().out
    assert calls[0]["method"] == "timeout"
    assert calls[0]["prefix"] == repl.REPL_MEMORY_GUARD_PREFIX
    assert calls[0]["env"]["MOLT_IO_MODE"] == "virtual"
    assert calls[0]["explicit"] is None
    assert calls[0]["default"] == repl.DEFAULT_REPL_TIMEOUT_SEC
    assert calls[0]["cwd"] == tmp_path
    assert calls[1]["method"] == "run"
    assert calls[1]["cmd"] == [
        "molt-dev",
        "run",
        str(snippet_paths[0]),
        "--capabilities",
        "fs.read",
    ]
    assert calls[1]["cwd"] == tmp_path
    assert calls[1]["memory_guard_prefix"] == repl.REPL_MEMORY_GUARD_PREFIX
    assert calls[1]["timeout"] == 12.5
    assert snippet_paths[0].parent == tmp_path / "tmp" / "repl"
    assert not snippet_paths[0].exists()


def test_run_repl_exits_cleanly_without_readline(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.chdir(tmp_path)
    real_import = builtins.__import__

    def fake_import(name: str, *args: object, **kwargs: object) -> object:
        if name == "readline":
            raise ImportError("readline unavailable")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    monkeypatch.setattr(builtins, "input", lambda _prompt="": "exit()")

    assert repl.run_repl(molt_cmd=["molt-dev"]) == 0
