from __future__ import annotations

import argparse
import ast
import importlib.util
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]


def _load_safe_run():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_safe_run", ROOT / "tools" / "safe_run.py"
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_safe_run_delegates_process_custody_to_memory_guard(tmp_path: Path) -> None:
    safe_run = _load_safe_run()
    ns = argparse.Namespace(rss_mb=512, timeout=7.5, poll=0.25)
    summary_path = tmp_path / "summary.json"

    command = safe_run._guard_command(
        ns,
        [sys.executable, "-c", "print('ok')"],
        summary_path,
    )

    assert command[:2] == [sys.executable, str(ROOT / "tools" / "memory_guard.py")]
    assert "--max-rss-gb" in command
    assert "--max-total-rss-gb" in command
    assert "--summary-json" in command
    assert str(summary_path) in command
    assert command[-4] == "--"
    assert command[-3:] == [sys.executable, "-c", "print('ok')"]


def test_safe_run_status_uses_guard_summary_before_legacy_exit_codes() -> None:
    safe_run = _load_safe_run()

    assert (
        safe_run._status(
            safe_run.EXIT_OOM,
            {
                "returncode": safe_run.EXIT_OOM,
                "timed_out": False,
                "violation": None,
            },
        )
        == "ok"
    )
    assert safe_run._status(0, {"timed_out": True, "violation": None}) == "timeout"
    assert safe_run._status(0, {"timed_out": False, "violation": {}}) == "oom"
    assert safe_run._status(safe_run.EXIT_OOM, {"peak": {}}) == "failed"
    assert safe_run._status(safe_run.EXIT_TIMEOUT, {}) == "failed"


def test_safe_run_preserves_guard_infrastructure_outcome(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    safe_run = _load_safe_run()
    summary_path = tmp_path / "summary.json"
    infrastructure_failure = {
        "phase": "temporary_artifact_custody",
        "details": ["scratch receipt retention failed"],
    }
    summary_path.write_text(
        json.dumps(
            {
                "returncode": safe_run.EXIT_SPAWN,
                "child_returncode": 0,
                "infrastructure_failure": infrastructure_failure,
                "elapsed_s": 0.25,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(safe_run, "_summary_path", lambda _label: summary_path)
    monkeypatch.setattr(
        safe_run.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 125),
    )

    returncode = safe_run.main(["--json", "--", "successful-child"])

    receipt = json.loads(capsys.readouterr().err.removeprefix("SAFE_RUN "))
    assert returncode == safe_run.EXIT_SPAWN
    assert receipt["status"] == "infrastructure_error"
    assert receipt["exit"] == safe_run.EXIT_SPAWN
    assert receipt["child_returncode"] == 0
    assert receipt["infrastructure_failure"] == infrastructure_failure
    assert receipt["infrastructure_failure_decode_error"] is None


def test_direct_run_tools_have_no_parallel_kill_authority() -> None:
    removed_compile_progress_helper = "_kill_run" + "_scoped_processes"
    for relative in ("tools/safe_run.py", "tools/compile_progress.py"):
        source = (ROOT / relative).read_text(encoding="utf-8")
        module = ast.parse(source)
        imported_modules = {
            alias.name
            for node in module.body
            if isinstance(node, ast.Import)
            for alias in node.names
        }
        process_kill_calls = [
            node
            for node in ast.walk(module)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "os"
            and node.func.attr in {"kill", "killpg"}
        ]

        assert "signal" not in imported_modules
        assert "killpg" not in source
        assert removed_compile_progress_helper not in source
        assert not process_kill_calls
