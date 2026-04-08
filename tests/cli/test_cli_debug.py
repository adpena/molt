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


def _run_cli(args: list[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=cwd,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _to_abs(path_str: str, *, cwd: Path) -> Path:
    path = Path(path_str)
    if path.is_absolute():
        return path
    return cwd / path


def test_debug_help_lists_canonical_subcommands(tmp_path: Path) -> None:
    res = _run_cli(["debug", "--help"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr
    for subcommand in ("repro", "ir", "verify", "trace", "reduce", "bisect", "diff", "perf"):
        assert subcommand in res.stdout


def test_debug_ir_and_verify_help_exist(tmp_path: Path) -> None:
    ir_help = _run_cli(["debug", "ir", "--help"], cwd=tmp_path)
    assert ir_help.returncode == 0, ir_help.stderr
    assert "usage:" in ir_help.stdout.lower()

    verify_help = _run_cli(["debug", "verify", "--help"], cwd=tmp_path)
    assert verify_help.returncode == 0, verify_help.stderr
    assert "usage:" in verify_help.stdout.lower()


def test_debug_command_writes_manifest_under_tmp_debug_by_default(tmp_path: Path) -> None:
    res = _run_cli(["debug", "ir", "--format", "json"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)

    manifest_path = _to_abs(payload["manifest_path"], cwd=tmp_path)
    assert manifest_path.is_file()
    assert manifest_path.parent.name
    assert manifest_path.parent.parent.name == "ir"
    assert manifest_path.parent.parent.parent.name == "debug"
    assert manifest_path.parent.parent.parent.parent.name == "tmp"
    assert manifest_path.parent.parent.parent.parent.parent.samefile(tmp_path)

    manifest_payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_payload["command"] == "debug"
    assert manifest_payload["subcommand"] == "ir"


def test_debug_command_out_redirects_artifacts_under_logs_debug(tmp_path: Path) -> None:
    out_path = tmp_path / "logs" / "debug" / "verify" / "summary.json"
    res = _run_cli(
        ["debug", "verify", "--format", "json", "--out", str(out_path)],
        cwd=tmp_path,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)

    retained_output = _to_abs(payload["artifacts"]["retained_output"], cwd=tmp_path)
    assert retained_output.is_file()
    assert retained_output.parent.name
    assert retained_output.parent.parent.name == "verify"
    assert retained_output.parent.parent.parent.name == "debug"
    assert retained_output.parent.parent.parent.parent.name == "logs"
    assert retained_output.parent.parent.parent.parent.parent.samefile(tmp_path)

    manifest_path = _to_abs(payload["manifest_path"], cwd=tmp_path)
    assert manifest_path.is_file()
    assert manifest_path.parent.name
    assert manifest_path.parent.parent.name == "verify"
    assert manifest_path.parent.parent.parent.name == "debug"
    assert manifest_path.parent.parent.parent.parent.name == "logs"
    assert manifest_path.parent.parent.parent.parent.parent.samefile(tmp_path)
