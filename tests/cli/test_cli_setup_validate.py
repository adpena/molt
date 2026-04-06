from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


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
    return subprocess.run(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _run_dev(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
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
    assert "cli-build-json" in names
    assert "cli-compare-json" in names
    assert "cli-unsupported-dynamic" in names
    assert "native-parity" in names
    assert "wasm-parity" in names
    assert "conformance-smoke" in names
    assert "bench-smoke" in names


def test_tools_dev_validate_delegates_to_canonical_cli() -> None:
    res = _run_dev(["validate", "--check"])
    assert res.returncode == 0, res.stderr
    assert "validate" in res.stdout.lower() or "validate" in res.stderr.lower()


def test_install_wrappers_delegate_into_setup() -> None:
    shell_text = (ROOT / "packaging" / "install.sh").read_text(encoding="utf-8")
    powershell_text = (ROOT / "packaging" / "install.ps1").read_text(encoding="utf-8")
    assert "molt setup" in shell_text
    assert "molt setup" in powershell_text.lower()
