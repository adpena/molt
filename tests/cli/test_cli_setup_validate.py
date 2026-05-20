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
