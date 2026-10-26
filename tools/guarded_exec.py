#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Mapping
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import harness_memory_guard  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]


from molt.cargo_execution_policy import admit_cargo_build  # noqa: E402


def _timeout_from_env(name: str | None, env: Mapping[str, str]) -> float | None:
    if not name:
        return None
    raw = env.get(name, "").strip()
    if not raw:
        return None
    try:
        parsed = float(raw)
    except ValueError:
        return None
    return parsed if parsed > 0 else None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command under Molt's canonical harness memory guard."
    )
    parser.add_argument("--prefix", default="MOLT")
    parser.add_argument("--cwd", type=Path, default=ROOT)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--timeout-env", default=None)
    parser.add_argument("--metrics-json", type=Path)
    parser.add_argument("--cancel-file", type=Path)
    parser.add_argument("--progress-file", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    maintenance_cwd = args.cwd.resolve()
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        command = (
            harness_memory_guard.memory_guard._load_internal_command(os.environ) or []
        )
    if not command:
        parser.error("command is required after --")

    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
    try:
        admit_cargo_build(command, cwd=args.cwd, env=env)
    except ValueError as exc:  # DiskCapacityError, or an inconsistent target
        print(f"guarded_exec preflight: {exc}", file=sys.stderr)
        return 2

    context = harness_memory_guard.HarnessExecutionContext.from_env(
        args.prefix,
        env,
        repo_root=ROOT,
    )
    timeout = harness_memory_guard.timeout_from_env(
        args.prefix,
        env,
        explicit=args.timeout,
        default=_timeout_from_env(args.timeout_env, env),
    )
    worker_env = dict(env)
    env = harness_memory_guard.memory_guard._child_env_without_internal_keys(env)
    closure_progress = (
        None
        if args.progress_file is None
        else harness_memory_guard.memory_guard.closure_progress_notifier(
            args.progress_file
        )
    )
    result = context.run(
        command,
        cwd=args.cwd,
        env=env,
        capture_output=False,
        timeout=timeout,
        cancellation_requested=(
            None if args.cancel_file is None else args.cancel_file.exists
        ),
        closure_progress=closure_progress,
        running_summary_json=(
            None if args.metrics_json is None else str(args.metrics_json)
        ),
        running_summary_environ=worker_env,
    )
    if args.metrics_json is not None:
        peak = getattr(result, "peak", None)
        peak_total = getattr(result, "peak_total", None)
        metrics = {
            "schema": "molt.guarded-command-metrics.v1",
            "returncode": int(result.returncode),
            "launch_id": worker_env.get(
                harness_memory_guard.memory_guard._cli_contract.INTERNAL_LAUNCH_ID_ENV
            ),
            "guard_pid": os.getpid(),
            "command": list(result.args),
            "child_process": harness_memory_guard.memory_guard.guarded_child_process_payload(
                result.child_process
            ),
            "cancelled": result.cancelled,
            "descendants_closed": result.descendants_closed,
            "child_returncode": getattr(result, "child_returncode", None),
            "infrastructure_failure": harness_memory_guard.memory_guard.infrastructure_failure_payload(
                getattr(result, "infrastructure_failure", None)
            ),
            "temporary_artifacts": getattr(result, "temporary_artifacts", None),
            "timed_out": bool(getattr(result, "timed_out", False)),
            "exit_signal": (
                None
                if result.cancelled
                or getattr(result, "timed_out", False)
                or getattr(result, "violation", None) is not None
                or getattr(result, "guard_signal", None) is not None
                else harness_memory_guard.memory_guard.exit_signal_payload(
                    int(result.returncode)
                )
            ),
            "memory_violation": harness_memory_guard._rss_record_payload(
                getattr(result, "violation", None)
            ),
            "guard_signal": getattr(result, "guard_signal", None),
            "termination_reports": harness_memory_guard.memory_guard.termination_reports_payload(
                getattr(result, "termination_reports", ())
            ),
            "cargo_incremental_quarantine": harness_memory_guard.memory_guard._cargo_incremental_quarantine_payload(
                getattr(result, "cargo_incremental_quarantine", None)
            ),
            "duration_seconds": getattr(result, "elapsed_s", None),
            "peak_process_rss_bytes": (
                int(peak.rss_kb) * 1024
                if peak is not None and getattr(peak, "rss_kb", None) is not None
                else None
            ),
            "peak_tree_rss_bytes": (
                int(peak_total.rss_kb) * 1024
                if peak_total is not None
                and getattr(peak_total, "rss_kb", None) is not None
                else None
            ),
            "peak_job_commit_bytes": getattr(
                result,
                "peak_job_commit_bytes",
                None,
            ),
            "worker_peaks": (
                None
                if getattr(result, "worker_peaks", None) is None
                else result.worker_peaks.json_payload()
            ),
            "windows_job_cleanup": harness_memory_guard.memory_guard.windows_job_cleanup_payload(
                getattr(result, "windows_job_cleanup", None)
            ),
        }
        metrics_path = args.metrics_json.resolve()
        metrics_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = metrics_path.with_name(f".{metrics_path.name}.{os.getpid()}.tmp")
        temporary.write_text(
            json.dumps(metrics, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(metrics_path)
        if closure_progress is not None:
            closure_progress("metrics_published")
    if result.stderr:
        sys.stderr.write(str(result.stderr))
    profile_path = harness_memory_guard.command_profile_log_path(env)
    elapsed_s = getattr(result, "elapsed_s", None)
    elapsed = "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"
    print(
        "guarded_exec: "
        f"elapsed={elapsed} returncode={result.returncode} "
        f"profile={profile_path}",
        file=sys.stderr,
    )
    if not worker_env.get("MOLT_MEMORY_GUARD_STARTUP_JSON") and not worker_env.get(
        "MOLT_PROOF_QUEUE"
    ):
        from molt.dx import collect_checkout_state_after_result

        collect_checkout_state_after_result(worker_env, cwd=maintenance_cwd)
    return int(result.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
