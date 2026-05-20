from __future__ import annotations

import json
import subprocess

import pytest

from tools import gen_stdlib_module_union


def _stdlib_payload() -> str:
    return json.dumps(
        {
            "modules": ["abc", "sys"],
            "packages": ["asyncio"],
            "py_modules": ["abc", "asyncio.base_events"],
            "py_packages": ["asyncio"],
        }
    )


def test_capture_version_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout=_stdlib_payload(), stderr="")

    monkeypatch.setattr(
        gen_stdlib_module_union.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    modules, packages, py_modules, py_packages = (
        gen_stdlib_module_union._capture_version("3.12")
    )

    assert modules == ("abc", "sys")
    assert packages == ("asyncio",)
    assert py_modules == ("abc", "asyncio.base_events")
    assert py_packages == ("asyncio",)
    assert captured["cmd"][:4] == ["uv", "run", "--python", "3.12"]
    assert captured["kwargs"]["prefix"] == "MOLT_TEST_SUITE"
    assert captured["kwargs"]["cwd"] == gen_stdlib_module_union.ROOT
    assert captured["kwargs"]["capture_output"] is True


def test_capture_version_preserves_check_output_failure(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 9, stdout="partial", stderr="oom")

    monkeypatch.setattr(
        gen_stdlib_module_union.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(subprocess.CalledProcessError) as exc_info:
        gen_stdlib_module_union._capture_version("3.14")

    assert exc_info.value.returncode == 9
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "oom"
