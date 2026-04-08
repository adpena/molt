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


def _write_source(tmp_path: Path) -> Path:
    source_path = tmp_path / "sample_debug_cli_input.py"
    source_path.write_text("print('debug-cli')\n", encoding="utf-8")
    return source_path


def _write_diff_inputs(tmp_path: Path) -> tuple[Path, Path]:
    summary_path = tmp_path / "summary.json"
    summary_path.write_text(
        json.dumps(
            {
                "run_id": "diff-run-123",
                "jobs": 2,
                "config": {"build_profile": "dev", "backend": "native"},
                "discovered": 3,
                "total": 3,
                "passed": 2,
                "failed": 1,
                "skipped": 0,
                "oom": 0,
                "failed_files": ["tests/differential/basic/example.py"],
            }
        ),
        encoding="utf-8",
    )
    failure_queue_path = tmp_path / "failures.txt"
    failure_queue_path.write_text(
        "tests/differential/basic/example.py\n# ignored comment\n",
        encoding="utf-8",
    )
    return summary_path, failure_queue_path


def _write_perf_inputs(tmp_path: Path) -> tuple[Path, Path]:
    profile_json_path = tmp_path / "bench_a.json"
    profile_json_path.write_text(
        json.dumps(
            {
                "profile": {"alloc_count": 4},
                "hot_paths": {"call_bind_ic_hit": 10, "call_bind_ic_miss": 1},
                "deopt_reasons": {},
            }
        ),
        encoding="utf-8",
    )
    profile_log_path = tmp_path / "bench_b.log"
    profile_log_path.write_text(
        "noise\nmolt_profile_json "
        + json.dumps(
            {
                "profile": {"alloc_count": 6, "alloc_callargs": 2},
                "hot_paths": {"call_bind_ic_hit": 3, "call_bind_ic_miss": 4},
                "deopt_reasons": {},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    return profile_json_path, profile_log_path


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

    diff_help = _run_cli(["debug", "diff", "--help"], cwd=tmp_path)
    assert diff_help.returncode == 0, diff_help.stderr
    assert "usage:" in diff_help.stdout.lower()

    perf_help = _run_cli(["debug", "perf", "--help"], cwd=tmp_path)
    assert perf_help.returncode == 0, perf_help.stderr
    assert "usage:" in perf_help.stdout.lower()


def test_debug_unwired_flows_accept_input_paths_and_emit_structured_payloads(
    tmp_path: Path,
) -> None:
    source_path = _write_source(tmp_path)

    for subcommand in ("repro", "trace", "reduce", "bisect"):
        res = _run_cli(
            ["debug", subcommand, str(source_path), "--format", "json"],
            cwd=tmp_path,
        )
        assert res.returncode == 0, res.stderr
        payload = json.loads(res.stdout)
        assert payload["subcommand"] == subcommand
        assert payload["status"] == "unsupported"
        assert payload["failure_class"] == "not_yet_wired"
        assert f"molt debug {subcommand} is not yet wired" == payload["message"]


def test_debug_command_writes_manifest_under_tmp_debug_by_default(tmp_path: Path) -> None:
    source_path = _write_source(tmp_path)
    res = _run_cli(["debug", "ir", str(source_path), "--format", "json"], cwd=tmp_path)
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


def test_debug_diff_consumes_summary_and_failure_queue(tmp_path: Path) -> None:
    summary_path, failure_queue_path = _write_diff_inputs(tmp_path)

    res = _run_cli(
        [
            "debug",
            "diff",
            str(summary_path),
            "--failure-queue",
            str(failure_queue_path),
            "--format",
            "json",
        ],
        cwd=tmp_path,
    )

    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["subcommand"] == "diff"
    assert payload["status"] == "ok"
    assert payload["failure_class"] is None
    assert payload["data"] == {
        "run_id": "diff-run-123",
        "jobs": 2,
        "counts": {
            "discovered": 3,
            "total": 3,
            "passed": 2,
            "failed": 1,
            "skipped": 0,
            "oom": 0,
        },
        "config": {"build_profile": "dev", "backend": "native"},
        "failed_files": ["tests/differential/basic/example.py"],
        "failure_queue": ["tests/differential/basic/example.py"],
    }

    manifest_path = _to_abs(payload["manifest_path"], cwd=tmp_path)
    assert manifest_path.is_file()
    manifest_payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_payload["subcommand"] == "diff"
    assert manifest_payload["data"] == payload["data"]


def test_debug_perf_consumes_profile_logs_and_retains_summary(tmp_path: Path) -> None:
    profile_json_path, profile_log_path = _write_perf_inputs(tmp_path)
    out_path = tmp_path / "logs" / "debug" / "perf" / "summary.json"

    res = _run_cli(
        [
            "debug",
            "perf",
            str(profile_json_path),
            str(profile_log_path),
            "--format",
            "json",
            "--out",
            str(out_path),
        ],
        cwd=tmp_path,
    )

    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["subcommand"] == "perf"
    assert payload["status"] == "ok"
    assert payload["failure_class"] is None
    assert payload["data"]["profile_count"] == 2
    assert payload["data"]["aggregate"]["hot_paths"] == {
        "call_bind_ic_hit": 13,
        "call_bind_ic_miss": 5,
    }
    assert payload["data"]["aggregate"]["allocations"] == {
        "alloc_count": 10,
        "alloc_callargs": 2,
    }
    assert payload["data"]["recommendations"]

    retained_output = _to_abs(payload["artifacts"]["retained_output"], cwd=tmp_path)
    assert retained_output.is_file()
    retained_payload = json.loads(retained_output.read_text(encoding="utf-8"))
    assert retained_payload["data"] == payload["data"]
