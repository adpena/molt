from __future__ import annotations

from pathlib import Path
import sys

from tools import check_memory_guard_wiring
from tools import memory_guard
from tools import pytest_memory_guard_bootstrap

REPO_ROOT = Path(__file__).resolve().parents[1]


def test_default_memory_guard_wiring_for_harness_entrypoints() -> None:
    audit = check_memory_guard_wiring.audit_repo()

    assert audit.missing_paths == ()
    assert audit.missing_tokens == ()
    assert audit.required_sentinel_missing == ()
    assert audit.sentinel_drift == ()
    assert audit.ok is True


def test_wiring_audit_locks_down_pytest_and_ci_gate_custody() -> None:
    contracts = {
        contract.path: contract.tokens
        for contract in check_memory_guard_wiring.PYTHON_GUARD_CONTRACTS
    }

    assert contracts["pyproject.toml"] == (
        "molt.pytest_memory_guard_bootstrap",
        "molt.pytest_memory_guard_config_plugin",
    )
    assert contracts["src/molt/pytest_memory_guard_bootstrap.py"] == (
        "tools.pytest_memory_guard_bootstrap",
        "pytest_load_initial_conftests",
    )
    assert contracts["src/molt/pytest_memory_guard_config_plugin.py"] == (
        "pytest_load_initial_conftests",
    )
    assert contracts["sitecustomize.py"] == ("ensure_python_test_memory_guard",)
    assert contracts["tools/pytest_memory_guard_bootstrap.py"] == (
        "MOLT_MEMORY_GUARD_ACTIVE",
        "MOLT_MEMORY_GUARD_PID",
        "MOLT_PYTEST_OUTER_GUARD_REEXEC",
        "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC",
        "tools/memory_guard.py",
        "MOLT_TEST_SUITE",
        "--noconftest",
        "--confcutdir",
        "sample_processes",
        "os.execvpe",
    )
    assert contracts["tests/conftest.py"] == (
        "harness_memory_guard",
        "repo_process_sentinel",
        "limits_from_env",
        "MOLT_PYTEST",
        "drain_on_exit=True",
    )
    assert contracts["tools/ci_gate.py"] == (
        "harness_memory_guard",
        "guarded_completed_process",
        "_resolve_memory_limits",
        "compile_governor.compile_slot",
        "MOLT_CI_GATE",
        "guarded_exec.py",
    )


def test_legacy_shell_entrypoints_enter_guarded_python_wrappers() -> None:
    missing_paths, missing_tokens = check_memory_guard_wiring._audit_token_contracts(
        check_memory_guard_wiring.REPO_ROOT,
        check_memory_guard_wiring.SHELL_WRAPPER_CONTRACTS,
    )

    assert missing_paths == ()
    assert missing_tokens == ()


def test_pytest_startup_reexecs_direct_pytest_under_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(72)

    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_pytest_memory_guard(
            orig_argv=[sys.executable, "-m", "pytest", "tests/test_one.py", "-q"],
            runtime_argv=["-m", "tests/test_one.py", "-q"],
        )
    except SystemExit as exc:
        assert exc.code == 72
    else:  # pragma: no cover
        raise AssertionError("expected pytest guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert "--summary-json" in argv
    assert argv[-5:] == [sys.executable, "-m", "pytest", "tests/test_one.py", "-q"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_PYTEST_OUTER_GUARD_REEXEC"] == "1"


def test_repo_test_script_startup_reexecs_under_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}
    script = REPO_ROOT / "tests" / "e2e" / "test_performance_guard.py"

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(74)

    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_repo_test_script_memory_guard(
            runtime_argv=[str(script), "--flag"],
        )
    except SystemExit as exc:
        assert exc.code == 74
    else:  # pragma: no cover
        raise AssertionError("expected repo test script guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert "--summary-json" in argv
    assert argv[-3:] == [sys.executable, str(script), "--flag"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"] == "1"


def test_repo_test_script_startup_ignores_non_test_scripts(tmp_path: Path) -> None:
    script = tmp_path / "script.py"
    script.write_text("print('not a repo test')\n", encoding="utf-8")

    assert (
        pytest_memory_guard_bootstrap.repo_test_script_invocation_args(
            runtime_argv=[str(script)]
        )
        is None
    )


def test_pytest_startup_detects_console_script_and_module_invocations() -> None:
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.pytest_invocation_args(
            orig_argv=[sys.executable, "-m", "pytest"],
            runtime_argv=["-m"],
        )
        == ()
    )
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-u", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-X", "dev", "-I", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-S", "-Xdev", "-m", "pytest", "tests"],
        runtime_argv=["-m", "tests"],
    ) == ("tests",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
        runtime_argv=[str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.pytest_invocation_args(
            orig_argv=[sys.executable, "tools/memory_guard.py"],
            runtime_argv=["tools/memory_guard.py"],
        )
        is None
    )


def test_pytest_initial_conftest_hook_reexecs_from_pytest_args(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(73)

    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)

    try:
        pytest_memory_guard_bootstrap.pytest_load_initial_conftests(
            object(),
            object(),
            ["tests/test_one.py", "-q"],
        )
    except SystemExit as exc:
        assert exc.code == 73
    else:  # pragma: no cover
        raise AssertionError("expected pytest hook guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[-5:] == [sys.executable, "-m", "pytest", "tests/test_one.py", "-q"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_PYTEST_OUTER_GUARD_REEXEC"] == "1"


def test_pytest_startup_rejects_hook_disabling_flags() -> None:
    for args in (
        ("--noconftest",),
        ("--confcutdir", str(REPO_ROOT.parent)),
        (f"--confcutdir={REPO_ROOT.parent}",),
    ):
        try:
            pytest_memory_guard_bootstrap.validate_pytest_guardable_args(args)
        except SystemExit:
            pass
        else:  # pragma: no cover
            raise AssertionError(f"expected pytest args to be rejected: {args}")


def test_pytest_startup_allows_repo_confcutdir() -> None:
    pytest_memory_guard_bootstrap.validate_pytest_guardable_args(
        ("--confcutdir", str(REPO_ROOT))
    )
    pytest_memory_guard_bootstrap.validate_pytest_guardable_args(
        (f"--confcutdir={REPO_ROOT / 'tests'}",)
    )


def test_outer_memory_guard_fails_closed_on_forged_or_unsampled_marker(
    monkeypatch,
) -> None:
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {"MOLT_MEMORY_GUARD_ACTIVE": "1", "MOLT_MEMORY_GUARD_PID": "123"}
        )
        is False
    )


def test_outer_memory_guard_requires_live_repo_memory_guard_ancestor(
    monkeypatch,
) -> None:
    samples = {
        100: memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            rss_kb=1,
            command=f"{sys.executable} {REPO_ROOT / 'tools' / 'memory_guard.py'} --",
        ),
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=100,
            rss_kb=1,
            command="uv run --python 3.12 pytest",
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=200,
            rss_kb=1,
            command=f"{sys.executable} -m pytest",
        ),
    }

    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "getpid", lambda: 300)

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {"MOLT_MEMORY_GUARD_ACTIVE": "1", "MOLT_MEMORY_GUARD_PID": "100"}
        )
        is True
    )
