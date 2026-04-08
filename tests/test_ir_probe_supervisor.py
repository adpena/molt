from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

from molt.debug.bisect import (
    ProbeSupervisorAttemptConfig,
    build_probe_supervisor_attempts,
    render_probe_supervisor_markdown,
    should_retry_probe_statuses,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "ir_probe_supervisor.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("ir_probe_supervisor", SCRIPT_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_dry_run_prints_attempts_and_build_timeout_env(
    tmp_path: Path, capsys, monkeypatch
) -> None:
    module = _load_module()
    argv = [
        "ir_probe_supervisor.py",
        "--dry-run",
        "--python",
        "3.12",
        "--jobs",
        "2",
        "--retry-jobs",
        "1",
        "--diff-build-timeout",
        "900",
        "--run-root-base",
        str(tmp_path / "runs"),
        "--cache-root",
        str(tmp_path / "cache"),
    ]
    monkeypatch.setattr(sys, "argv", argv)

    rc = module.main()
    out = capsys.readouterr().out

    assert rc == 0
    assert "attempt 1:" in out
    assert "attempt 2:" in out
    assert "dry-run env: MOLT_DIFF_BUILD_TIMEOUT=900" in out
    assert "ir-probe-supervisor: dry-run complete" in out


def test_shared_probe_supervisor_helpers_match_wrapper_contract() -> None:
    attempts = build_probe_supervisor_attempts(
        jobs=2,
        retry_jobs=1,
        run_timeout=5400,
    )

    assert attempts == (
        ProbeSupervisorAttemptConfig(attempt=1, jobs=2, timeout_sec=5400),
        ProbeSupervisorAttemptConfig(attempt=2, jobs=1, timeout_sec=5400),
    )
    assert should_retry_probe_statuses({"probe.py": "build_timeout"}) is True
    assert should_retry_probe_statuses({"probe.py": "run_timeout"}) is True
    assert should_retry_probe_statuses({"probe.py": "build_failed"}) is True
    assert should_retry_probe_statuses({"probe.py": "ok"}) is False

    markdown = render_probe_supervisor_markdown(
        started_at="2026-04-08T12:00:00+00:00",
        finished_at="2026-04-08T12:01:00+00:00",
        required_probes=("a.py", "b.py"),
        attempts=[
            {
                "attempt": 1,
                "jobs": 2,
                "timeout_sec": 5400,
                "diff_rc": 1,
                "gate_rc": 1,
                "timed_out": False,
                "run_id": "run-1",
            }
        ],
        final_ok=False,
        final_message="attempt 1 failed",
    )

    assert "# IR Probe Supervisor Checkpoint" in markdown
    assert "| Attempt | Jobs | Timeout | Diff RC | Gate RC | Timed Out | Run ID |" in markdown
    assert "| 1 | 2 | 5400s | 1 | 1 | no | run-1 |" in markdown
