from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


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


def _run_cli(args: list[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=cwd,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _load_verify_module():
    try:
        return importlib.import_module("molt.debug.verify")
    except ModuleNotFoundError as exc:
        pytest.fail(f"molt.debug.verify is not available yet: {exc}")


def test_debug_verify_json_exposes_ir_inventory_and_probe_checks(tmp_path: Path) -> None:
    res = _run_cli(["debug", "verify", "--format", "json"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr

    payload = json.loads(res.stdout)
    assert payload["subcommand"] == "verify"
    assert payload["status"] == "ok"

    check_names = [entry["name"] for entry in payload["data"]["checks"]]
    assert "ir-inventory" in check_names
    assert "required-diff-probes" in check_names

    manifest_path = Path(payload["manifest_path"])
    assert manifest_path.is_file()
    manifest_payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_payload["data"]["checks"] == payload["data"]["checks"]


def test_verify_result_payload_includes_function_pass_and_artifact_references() -> None:
    module = _load_verify_module()

    finding = module.VerificationFinding(
        verifier="ir-inventory",
        message="dangling SSA value",
        function="selected",
        pass_name="verifier",
        artifact="tmp/debug/ir/selected.json",
        severity="error",
    )
    payload = module.build_verify_result_payload(
        checks=[
            {
                "name": "ir-inventory",
                "status": "error",
                "findings": [finding],
            }
        ]
    )

    findings = payload["checks"][0]["findings"]
    assert findings == [
        {
            "verifier": "ir-inventory",
            "severity": "error",
            "message": "dangling SSA value",
            "function": "selected",
            "pass": "verifier",
            "artifact": "tmp/debug/ir/selected.json",
        }
    ]
