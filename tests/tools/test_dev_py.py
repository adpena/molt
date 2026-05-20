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


def test_dev_py_clean_artifacts_dispatches_to_cleanup_tool(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[list[str]] = []
    create_dirs_values: list[bool] = []

    def fake_canonical_env(*, create_dirs=True):
        create_dirs_values.append(create_dirs)
        return {"PATH": "", "PYTHONPATH": str(module.ROOT / "src")}

    monkeypatch.setattr(
        module,
        "_canonical_env",
        fake_canonical_env,
        raising=True,
    )
    monkeypatch.setattr(
        module,
        "_run_repo_cmd",
        lambda cmd, _env, *, tty: calls.append(list(cmd)),
        raising=True,
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "clean-artifacts", "--apply"],
        raising=True,
    )

    module.main()

    assert calls == [
        [
            str(module.DX.project_python()),
            "tools/artifact_cleanup.py",
            "--apply",
        ]
    ]
    assert create_dirs_values == [False]


def test_dev_py_lint_uses_documented_stdlib_intrinsic_gates(monkeypatch) -> None:
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
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "lint"],
        raising=True,
    )
    module.main()

    stdlib_calls = [
        args
        for args in calls
        if len(args) > 1 and args[1] == "tools/check_stdlib_intrinsics.py"
    ]

    assert any("--fallback-intrinsic-backed-only" in args for args in stdlib_calls)
    assert any("--critical-allowlist" in args for args in stdlib_calls)
    assert calls[0][1:4] == ["-m", "ruff", "check"]
    assert calls[1][1:5] == ["-m", "ruff", "format", "--check"]


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


def test_dev_py_canonical_env_keeps_backend_daemon_enabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_dev_py()
    monkeypatch.delenv("MOLT_BACKEND_DAEMON", raising=False)

    env = module._canonical_env()

    assert env["MOLT_BACKEND_DAEMON"] == "1"


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


def test_dev_py_tty_uses_guard_when_memory_guard_enabled(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[str, list[str]]] = []

    def fake_check_call_guarded(cmd, env, *, limits=None):
        calls.append(("guarded", list(cmd)))

    def fail_pty(cmd, env):
        raise AssertionError("PTY path must not bypass memory guard")

    monkeypatch.setattr(module, "_check_call_guarded", fake_check_call_guarded)
    monkeypatch.setattr(module, "_run_with_pty", fail_pty)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "1"}, tty=True
    )

    assert calls == [("guarded", ["pytest", "-q"])]


def test_dev_py_tty_can_use_pty_when_memory_guard_disabled(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[str, list[str]]] = []

    def fail_guarded(cmd, env, *, limits=None):
        raise AssertionError("guarded path should not run when guard is disabled")

    def fake_pty(cmd, env):
        calls.append(("pty", list(cmd)))

    monkeypatch.setattr(module, "_check_call_guarded", fail_guarded)
    monkeypatch.setattr(module, "_run_with_pty", fake_pty)

    module._run_repo_cmd(
        ["pytest", "-q"], {"MOLT_TEST_SUITE_MEMORY_GUARD": "0"}, tty=True
    )

    if module.os.name == "posix":
        assert calls == [("pty", ["pytest", "-q"])]
    else:
        assert calls == []
