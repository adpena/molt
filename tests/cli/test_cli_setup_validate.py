from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[2]


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _run_cli(args: list[str]) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _run_dev(args: list[str]) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [_python_executable(), "tools/dev.py", *args],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def test_cli_setup_json_reports_actions_and_environment() -> None:
    res = _run_cli(["setup", "--json"])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "setup"
    assert payload["status"] in {"ok", "error"}
    data = payload["data"]
    assert isinstance(data.get("checks"), list)
    assert isinstance(data.get("environment"), dict)
    assert isinstance(data.get("actions"), list)
    assert "CARGO_TARGET_DIR" in data["environment"]
    assert "MOLT_CACHE" in data["environment"]


def test_cli_validate_check_json_reports_canonical_matrix() -> None:
    res = _run_cli(["validate", "--check", "--json", "--suite", "smoke"])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["check_only"] is True
    steps = data["steps"]
    assert isinstance(steps, list)
    names = {entry["name"] for entry in steps}
    assert "cli-run-json" in names
    assert "cli-command-json" in names
    assert "native-parity" in names
    assert "wasm-parity" in names
    assert "conformance-smoke" in names
    assert "bench-smoke" in names
    cli_command_step = next(
        entry for entry in steps if entry["name"] == "cli-command-json"
    )
    cli_command_expr = cli_command_step["cmd"][cli_command_step["cmd"].index("-k") + 1]
    assert "test_cli_build_json_binary_executes_for_native_profiles" in cli_command_expr
    assert "test_cli_compare_json" in cli_command_expr
    assert "test_cli_run_exec_eval_raise_runtime_error" in cli_command_expr
    bench_step = next(entry for entry in steps if entry["name"] == "bench-smoke")
    assert "--warmup" in bench_step["cmd"]
    assert bench_step["cmd"][bench_step["cmd"].index("--warmup") + 1] == "1"


def test_cli_run_command_uses_memory_guard_prefix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    class FakeMemoryGuard:
        @staticmethod
        def limits_from_env(prefix: str, env: dict[str, str] | None) -> object:
            calls.append({"method": "limits", "prefix": prefix, "env": env})
            return object()

        @staticmethod
        def guarded_completed_process(cmd: list[str], **kwargs: object):
            calls.append({"method": "run", "cmd": cmd, **kwargs})
            return subprocess.CompletedProcess(cmd, 0, "stdout\n", "stderr\n")

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("guarded CLI command used raw subprocess.run")

    monkeypatch.setattr(
        cli,
        "_load_cli_harness_memory_guard",
        lambda cwd: FakeMemoryGuard,
        raising=True,
    )
    monkeypatch.setattr(cli.subprocess, "run", fail_raw_run, raising=True)

    rc = cli._run_command(
        ["python3", "-c", "print('ok')"],
        cwd=ROOT,
        env={"PATH": "/usr/bin"},
        memory_guard_prefix="MOLT_BENCH",
    )

    assert rc == 0
    assert calls[0] == {
        "method": "limits",
        "prefix": "MOLT_BENCH",
        "env": {"PATH": "/usr/bin"},
    }
    assert calls[1]["method"] == "run"
    assert calls[1]["prefix"] == "MOLT_BENCH"
    assert calls[1]["cwd"] == ROOT
    assert calls[1]["capture_output"] is False


def test_cli_bench_outer_process_uses_bench_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(cli, "_run_command", fake_run_command, raising=True)

    assert cli.bench(wasm=False, bench_args=["--smoke"]) == 0

    assert calls
    assert calls[0]["memory_guard_prefix"] == "MOLT_BENCH"
    assert "tools/bench.py" in calls[0]["cmd"]


def test_cli_validate_uses_family_memory_guard_prefixes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    prefixes: list[str] = []
    steps = [
        cli._ValidationStep(
            "conformance-step",
            ["python3", "-c", "pass"],
            ROOT,
            "conformance",
            ("native",),
            ("dev",),
            "smoke",
        ),
        cli._ValidationStep(
            "bench-step",
            ["python3", "-c", "pass"],
            ROOT,
            "benchmark",
            ("native",),
            ("dev",),
            "smoke",
        ),
        cli._ValidationStep(
            "correctness-step",
            ["python3", "-c", "pass"],
            ROOT,
            "correctness",
            ("native",),
            ("dev",),
            "smoke",
        ),
    ]

    class FakeMemoryGuard:
        @staticmethod
        def limits_from_env(prefix: str, env: dict[str, str] | None) -> object:
            del env
            return {"prefix": prefix}

        @staticmethod
        def guarded_completed_process(cmd: list[str], **kwargs: object):
            prefixes.append(str(kwargs["prefix"]))
            return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_find_molt_root", lambda *args: ROOT, raising=True)
    monkeypatch.setattr(
        cli,
        "_planned_validate_steps",
        lambda root, suite, backend, profile: steps,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_load_cli_harness_memory_guard",
        lambda cwd: FakeMemoryGuard,
        raising=True,
    )

    assert cli.validate(suite="smoke", json_output=True) == 0

    assert prefixes == ["MOLT_CONFORMANCE", "MOLT_BENCH", "MOLT_TEST_SUITE"]


def test_tools_dev_validate_delegates_to_canonical_cli() -> None:
    res = _run_dev(["validate", "--check"])
    assert res.returncode == 0, res.stderr
    assert "validate" in res.stdout.lower() or "validate" in res.stderr.lower()


def test_cli_lint_uses_shared_dx_planner(monkeypatch: pytest.MonkeyPatch) -> None:
    from molt import cli

    calls: list[list[str]] = []

    class FakeDxProject:
        def __init__(self, root: Path) -> None:
            self.root = root

        def canonical_env(self) -> dict[str, str]:
            return {"PATH": "", "PYTHONPATH": str(ROOT / "src")}

        def require_project_python(self, context: str) -> Path:
            assert context == "lint"
            return ROOT / ".venv" / "bin" / "python3"

        def commands(self) -> dict[str, object]:
            return {"lint": "python3 -m ruff check ."}

        def split_command_sequence(self, command: object, name: str) -> list[list[str]]:
            assert command == "python3 -m ruff check ."
            assert name == "lint"
            return [["python3", "-m", "ruff", "check", "."]]

    def fake_run(cmd, **kwargs):
        calls.append(list(cmd))
        assert cmd != [sys.executable, "tools/dev.py", "lint"]
        assert kwargs["cwd"] == ROOT
        assert kwargs["capture_output"] is False
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(cli, "DxProject", FakeDxProject, raising=True)
    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)

    assert cli.lint(json_output=False, verbose=False) == 0
    assert calls == [["python3", "-m", "ruff", "check", "."]]


def test_install_wrappers_delegate_into_setup() -> None:
    shell_text = (ROOT / "packaging" / "install.sh").read_text(encoding="utf-8")
    powershell_text = (ROOT / "packaging" / "install.ps1").read_text(encoding="utf-8")
    assert "molt setup" in shell_text
    assert "molt setup" in powershell_text.lower()
