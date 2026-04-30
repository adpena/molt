from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
DEV_PY = REPO_ROOT / "tools" / "dev.py"


def _load_dev_py():
    spec = importlib.util.spec_from_file_location("molt_tools_dev_py", DEV_PY)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_dev_py_update_dispatches_to_cli(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "update", "--check", "--all"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            ["python3", "-m", "molt.cli", "update", "--check", "--all"],
            module.TEST_PYTHONS[0],
            False,
        )
    ]


def test_dev_py_lint_uses_documented_stdlib_intrinsic_gates(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "lint"],
        raising=True,
    )
    module.main()

    stdlib_calls = [
        args
        for args, _python, _tty in calls
        if args[:2] == ["python3", "tools/check_stdlib_intrinsics.py"]
    ]

    assert [
        "python3",
        "tools/check_stdlib_intrinsics.py",
        "--fallback-intrinsic-backed-only",
    ] in stdlib_calls
    assert [
        "python3",
        "tools/check_stdlib_intrinsics.py",
        "--critical-allowlist",
    ] in stdlib_calls


def test_dev_py_gates_expand_pyproject_command_refs(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []

    monkeypatch.setattr(
        module,
        "_canonical_env",
        lambda: {"PATH": "", "PYTHONPATH": str(module.ROOT / "src")},
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_require_project_python",
        lambda: module.ROOT / ".venv" / "bin" / "python3",
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )

    def fake_status_run(cmd, **_kwargs):
        assert cmd == ["git", "status", "--short"]
        return SimpleNamespace(stdout="")

    monkeypatch.setattr(module.subprocess, "run", fake_status_run, raising=True)

    module._run_dx_gates(["--allow-dirty"], tty=False)

    assert calls[:2] == [
        [
            "cargo",
            "clippy",
            "-p",
            "molt-backend",
            "--features",
            "native-backend",
            "--",
            "-D",
            "warnings",
        ],
        ["cargo", "deny", "check"],
    ]
    assert calls[2][0:4] == ["cargo", "build", "--profile", "release-fast"]
    assert calls[3][0:6] == [
        "cargo",
        "test",
        "--profile",
        "release-fast",
        "-p",
        "molt-backend",
    ]
    assert calls[4][1:4] == ["-m", "pytest", "tests/compliance/"]


def test_dev_py_command_refs_fail_loudly_on_bad_config() -> None:
    module = _load_dev_py()

    with pytest.raises(RuntimeError, match="Missing .*missing"):
        module._split_command_sequence("@missing", "root", commands={})

    with pytest.raises(RuntimeError, match="Cyclic"):
        module._split_command_sequence("@a", "root", commands={"a": "@a"})


def test_dev_py_test_forwards_random_order_flags(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "test", "--random-order", "--random-seed", "17"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--verified-subset",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[0],
            False,
        ),
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[1],
            False,
        ),
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[2],
            False,
        ),
    ]
