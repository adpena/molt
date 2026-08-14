from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
import contextlib
import json
import os
from pathlib import Path
import signal
import sys
from typing import TextIO

from tools.memory_guard_core.cargo_quarantine import (
    _cargo_incremental_quarantine_message,
    _cargo_incremental_quarantine_payload,
)
from tools.memory_guard_core.common import utc_compact_timestamp, utc_timestamp
from tools.memory_guard_core.payloads import (
    _rss_record_payload,
    guarded_child_process_payload,
    memory_limits_payload,
    termination_reports_payload,
    windows_job_cleanup_payload,
)
from tools.memory_guard_core.process_custody import (
    GuardResult,
    GuardSamplingTelemetry,
    GuardTerminationAction,
    GuardedChildProcess,
)


WINDOWS_PROCESS_SIGNAL_EXIT_CODES = frozenset(
    code
    for code in (
        int(signal.SIGTERM),
        int(getattr(signal, "SIGBREAK", 0)),
    )
    if code > 0
)

_CONVENTIONAL_SIGNAL_NAMES = {
    1: "SIGHUP",
    2: "SIGINT",
    3: "SIGQUIT",
    6: "SIGABRT",
    9: "SIGKILL",
    15: "SIGTERM",
}


def exit_signal_payload(
    returncode: int,
    *,
    windows_process_model: bool,
) -> dict[str, object] | None:
    conventional_shell_status = False
    if returncode < 0:
        signo = -returncode
    elif 129 <= returncode <= 192:
        signo = returncode - 128
        conventional_shell_status = True
    elif windows_process_model and returncode in WINDOWS_PROCESS_SIGNAL_EXIT_CODES:
        signo = returncode
    else:
        return None
    with contextlib.suppress(ValueError):
        signame = signal.Signals(signo).name
        return {
            "signal": signo,
            "name": signame,
            "conventional_shell_status": conventional_shell_status,
        }
    return {
        "signal": signo,
        "name": _CONVENTIONAL_SIGNAL_NAMES.get(signo),
        "conventional_shell_status": conventional_shell_status,
    }


def sampling_telemetry_payload(
    telemetry: GuardSamplingTelemetry | None,
) -> dict[str, object] | None:
    if telemetry is None:
        return None
    return {
        "attempts": telemetry.attempts,
        "successes": telemetry.successes,
        "transient_failures": telemetry.transient_failures,
        "enforcement_complete": telemetry.enforcement_complete,
        "first_transient_failure_at": telemetry.first_transient_failure_at,
        "last_transient_failure_at": telemetry.last_transient_failure_at,
        "last_transient_error": telemetry.last_transient_error,
        "source": telemetry.source,
        "wall_time_s": telemetry.wall_time_s,
        "cpu_time_s": telemetry.cpu_time_s,
        "max_wall_time_s": telemetry.max_wall_time_s,
        "max_cpu_time_s": telemetry.max_cpu_time_s,
        "process_rows": telemetry.process_rows,
        "max_process_rows": telemetry.max_process_rows,
        "observer_wall_time_s": telemetry.observer_wall_time_s,
        "observer_cpu_time_s": telemetry.observer_cpu_time_s,
        "observer_cpu_duty_cycle": telemetry.observer_cpu_duty_cycle,
    }


def incident_payload(
    result: GuardResult,
    *,
    signal_payload: Callable[[int], dict[str, object] | None],
) -> dict[str, object] | None:
    orphan_reasons = {
        "tracked_orphan_cleanup",
        "repo_scoped_orphan_cleanup",
    }
    final_orphan_actions: dict[tuple[str, int], GuardTerminationAction] = {}
    final_primary_actions: dict[tuple[str, int], GuardTerminationAction] = {}
    for report in result.termination_reports:
        for action in report.actions:
            if (
                report.reason == "tracked_orphan_cleanup"
                and action.target_kind == "process"
                and action.target_id == report.root_pid
            ):
                continue
            target_kind = (
                "process"
                if action.target_kind == "owned_child_handle"
                else action.target_kind
            )
            key = (target_kind, action.target_id)
            if report.reason in orphan_reasons:
                final_orphan_actions[key] = action
            else:
                final_primary_actions[key] = action
    incomplete_orphan_actions = [
        action
        for action in final_orphan_actions.values()
        if action.result not in {"completed_or_missing", "missing"}
    ]
    incomplete_primary_actions = [
        action
        for action in final_primary_actions.values()
        if action.result not in {"completed_or_missing", "missing"}
    ]
    candidate_pids = sorted(
        {
            action.target_id
            for action in incomplete_orphan_actions
            if action.target_kind == "process"
        }
    )
    primary_candidate_pids = sorted(
        {
            action.target_id
            for action in incomplete_primary_actions
            if action.target_kind == "process"
        }
    )

    def cleanup_truth(default: str) -> str:
        if (
            result.windows_job_cleanup is not None
            and result.windows_job_cleanup.completed
        ):
            return default
        if not incomplete_orphan_actions and not incomplete_primary_actions:
            return default
        return (
            "process cleanup incomplete; no unverified process was reported as "
            "cleaned or used to trigger Cargo quarantine"
        )

    def attach_guard_custody(payload: dict[str, object]) -> dict[str, object]:
        child_payload = guarded_child_process_payload(result.child_process)
        if child_payload is not None:
            payload["child_process"] = child_payload
        if result.termination_reports:
            payload["termination_reports"] = termination_reports_payload(
                result.termination_reports
            )
        exact_job_completed = (
            result.windows_job_cleanup is not None
            and result.windows_job_cleanup.completed
        )
        if incomplete_orphan_actions and not exact_job_completed:
            payload["orphan_cleanup_status"] = "incomplete"
            payload["orphan_cleanup_candidate_pids"] = candidate_pids
        if incomplete_primary_actions and not exact_job_completed:
            payload["process_tree_cleanup_status"] = "incomplete"
            payload["process_tree_cleanup_candidate_pids"] = primary_candidate_pids
        return payload

    guard_signal_payload = (
        None
        if result.guard_signal is None
        else signal_payload(128 + result.guard_signal)
    )
    if (
        result.guard_signal is not None
        and result.violation is None
        and not result.timed_out
    ):
        payload: dict[str, object] = {
            "reason": "guard_interrupted",
            "cleanup": cleanup_truth(
                "terminated tracked process tree and post-baseline Molt process groups"
                if result.orphaned_process_groups
                else "terminated tracked process tree"
            ),
            "recorded_at": utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "signal": guard_signal_payload,
            "next_action": (
                "Inspect the parent host/control-plane signal source and child "
                "logs; the guard parent received the signal and wrote this "
                "summary before exiting."
            ),
        }
        if result.orphaned_process_groups:
            payload["process_groups"] = list(result.orphaned_process_groups)
        return attach_guard_custody(payload)
    if result.violation is not None:
        payload = {
            "reason": "rss_limit_exceeded",
            "cleanup": cleanup_truth(
                "classified command as failed from child exit resource usage"
                if result.violation.scope == "process_rusage"
                else "terminated tracked process tree"
            ),
            "recorded_at": utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "next_action": (
                "Inspect child logs and allocations, lower parallelism/input size, "
                "or raise the relevant memory guard RSS limit if the workload is "
                "expected."
            ),
        }
        if guard_signal_payload is not None:
            payload["guard_signal"] = guard_signal_payload
        return attach_guard_custody(payload)
    if result.timed_out:
        payload = {
            "reason": "timeout",
            "cleanup": cleanup_truth(
                "terminated tracked process tree and post-baseline Molt process groups"
                if result.orphaned_process_groups
                else "terminated tracked process tree"
            ),
            "recorded_at": utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "next_action": (
                "Inspect child logs for a hang or oversized workload; raise the "
                "guard timeout only for intentional long-running work."
            ),
        }
        if result.orphaned_process_groups:
            payload["process_groups"] = list(result.orphaned_process_groups)
        if guard_signal_payload is not None:
            payload["guard_signal"] = guard_signal_payload
        return attach_guard_custody(payload)
    if incomplete_orphan_actions:
        return attach_guard_custody(
            {
                "reason": "orphan_cleanup_incomplete",
                "cleanup": (
                    "no unverified process was reported as cleaned or used to "
                    "trigger Cargo quarantine"
                ),
                "recorded_at": utc_timestamp(),
                "elapsed_s": result.elapsed_s,
                "candidate_pids": candidate_pids,
                "next_action": (
                    "Inspect custody identities and termination actions; repair "
                    "the child lifecycle or sampler authority before retrying cleanup."
                ),
            }
        )
    if result.orphaned_process_groups:
        return attach_guard_custody(
            {
                "reason": "orphaned_processes_cleaned",
                "cleanup": "terminated tracked orphan descendants; group ids recorded",
                "recorded_at": utc_timestamp(),
                "elapsed_s": result.elapsed_s,
                "process_groups": list(result.orphaned_process_groups),
                "next_action": (
                    "Inspect child process lifecycle and logs; make helpers shut down "
                    "explicitly, or run intentional warm daemons inside a suite-level "
                    "sentinel that drains at scope exit."
                ),
            }
        )
    exit_signal = signal_payload(result.returncode)
    if exit_signal is not None:
        cleanup = (
            "quarantined Cargo incremental state"
            if result.cargo_incremental_quarantine is not None
            and result.cargo_incremental_quarantine.moved_paths
            else "none_by_guard"
        )
        return attach_guard_custody(
            {
                "reason": "signal_exit",
                "cleanup": cleanup,
                "recorded_at": utc_timestamp(),
                "elapsed_s": result.elapsed_s,
                "signal": exit_signal,
                "next_action": (
                    "Inspect child stderr/logs or the host signal source; the memory "
                    "guard did not classify this as an RSS limit trip."
                ),
            }
        )
    return None


def write_summary_json(
    path: str,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    max_global_rss_kb: int | None,
    child_rlimit_kb: int | None,
    timeout_s: float | None,
    poll_interval_s: float,
    result: GuardResult,
    signal_payload: Callable[[int], dict[str, object] | None],
    repro_context_provider: Callable[..., dict[str, object]],
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    incident = incident_payload(result, signal_payload=signal_payload)
    payload = {
        "command": list(command),
        "returncode": result.returncode,
        "elapsed_s": result.elapsed_s,
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "child_rlimit_kb": child_rlimit_kb,
        "child_rlimit_gb": (
            None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
        ),
        "violation": _rss_record_payload(result.violation),
        "peak": _rss_record_payload(result.peak),
        "peak_total": _rss_record_payload(result.peak_total),
        "peak_job_commit_bytes": result.peak_job_commit_bytes,
        "windows_job_cleanup": windows_job_cleanup_payload(result.windows_job_cleanup),
        "timed_out": result.timed_out,
        "orphaned_process_groups": list(result.orphaned_process_groups),
        "child_process": guarded_child_process_payload(result.child_process),
        "termination_reports": termination_reports_payload(result.termination_reports),
        "sampling_telemetry": sampling_telemetry_payload(result.sampling_telemetry),
        "cargo_incremental_quarantine": _cargo_incremental_quarantine_payload(
            result.cargo_incremental_quarantine
        ),
        "limit_at_violation": (
            None
            if result.limit_at_violation is None
            else memory_limits_payload(result.limit_at_violation)
        ),
        "exit_signal": (
            None
            if (
                result.violation is not None
                or result.timed_out
                or result.guard_signal is not None
            )
            else signal_payload(result.returncode)
        ),
        "guard_signal": (
            None
            if result.guard_signal is None
            else signal_payload(128 + result.guard_signal)
        ),
        "incident": incident,
    }
    if incident is not None:
        payload["repro"] = repro_context_provider(
            command=command,
            cwd=cwd,
            environ=environ,
            max_process_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
            max_global_rss_kb=max_global_rss_kb,
            child_rlimit_kb=child_rlimit_kb,
            timeout_s=timeout_s,
            poll_interval_s=poll_interval_s,
            summary_json=path,
            incident_pid=result.violation.pid if result.violation is not None else None,
        )
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def write_worker_exit_summary_json(
    path: str,
    *,
    worker_returncode: int,
    signal_payload: Callable[[int], dict[str, object] | None],
) -> bool:
    summary_path = Path(path)
    try:
        payload = json.loads(summary_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    if not isinstance(payload, dict):
        return False
    status = payload.get("status")
    if (
        status not in {"running", "child_running"}
        or payload.get("returncode") is not None
    ):
        return False

    recorded_at = utc_timestamp()
    exit_signal = signal_payload(worker_returncode)
    payload["status"] = "guard_worker_exited_without_final_summary"
    payload["returncode"] = worker_returncode
    payload["worker_returncode"] = worker_returncode
    payload["worker_exit_signal"] = exit_signal
    payload["recorded_at"] = recorded_at
    payload["incident"] = {
        "reason": "guard_worker_exited_without_final_summary",
        "cleanup": "none_by_wrapper",
        "recorded_at": recorded_at,
        "previous_status": status,
        "worker_returncode": worker_returncode,
        "worker_exit_signal": exit_signal,
        "next_action": (
            "Inspect the guard worker, child logs, and parent host/control-plane "
            "signal source. The public memory_guard wrapper preserved terminal "
            "custody because the internal worker exited before writing the final "
            "summary."
        ),
    }
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return True


def default_incident_summary_path(root: Path) -> Path:
    stamp = utc_compact_timestamp()
    return (
        root / "tmp" / "memory_guard" / "incidents" / f"{stamp}-pid{os.getpid()}.json"
    )


def prune_default_incident_summaries(directory: Path, *, keep: int) -> None:
    if keep <= 0:
        return
    try:
        paths = sorted(
            (path for path in directory.glob("*.json") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
    except OSError:
        return
    for path in paths[keep:]:
        with contextlib.suppress(OSError):
            path.unlink()


def write_running_summary_json(
    path: str,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    max_global_rss_kb: int | None,
    child_rlimit_kb: int | None,
    timeout_s: float | None,
    poll_interval_s: float,
    child_process: GuardedChildProcess | None = None,
    repro_context_provider: Callable[..., dict[str, object]],
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    child_payload = guarded_child_process_payload(child_process)
    payload = {
        "command": list(command),
        "returncode": None,
        "recorded_at": utc_timestamp(),
        "status": "running",
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "child_rlimit_kb": child_rlimit_kb,
        "child_rlimit_gb": (
            None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
        ),
        "violation": None,
        "peak": None,
        "peak_total": None,
        "peak_job_commit_bytes": None,
        "timed_out": False,
        "orphaned_process_groups": [],
        "child_process": child_payload,
        "termination_reports": [],
        "sampling_telemetry": None,
        "cargo_incremental_quarantine": None,
        "limit_at_violation": None,
        "exit_signal": None,
        "guard_signal": None,
        "incident": {
            "reason": "child_running" if child_payload is not None else "guard_started",
            "cleanup": "pending",
            "recorded_at": utc_timestamp(),
            "next_action": (
                "If this file remains in running status, the guard parent was "
                "terminated before it could write the final summary; use the "
                "child_process identity, repro block, and host/control-plane "
                "samples below."
            ),
        },
        "repro": repro_context_provider(
            command=command,
            cwd=cwd,
            environ=environ,
            max_process_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
            max_global_rss_kb=max_global_rss_kb,
            child_rlimit_kb=child_rlimit_kb,
            timeout_s=timeout_s,
            poll_interval_s=poll_interval_s,
            summary_json=path,
            incident_pid=None if child_process is None else child_process.pid,
        ),
    }
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def limit_text(limit_gb: float | None) -> str:
    return "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"


def child_identity_text(child: GuardedChildProcess | None) -> str:
    if child is None:
        return "child_pid=unknown child_pgid=unknown child_sid=unknown"
    return f"child_pid={child.pid} child_pgid={child.pgid} child_sid={child.sid}"


def emit_terminal_report(
    result: GuardResult,
    *,
    timeout_s: float | None,
    max_rss_gb: float,
    max_total_rss_gb: float,
    repro_payload: Mapping[str, object] | None,
    signal_payload: Callable[[int], dict[str, object] | None],
    stderr: TextIO = sys.stderr,
) -> None:
    if result.violation is not None:
        violation_limits = result.limit_at_violation
        limit_gb = (
            (
                violation_limits.max_total_rss_gb
                if violation_limits is not None
                else max_total_rss_gb
            )
            if result.violation.scope == "process_tree"
            else (
                violation_limits.max_process_rss_gb
                if violation_limits is not None
                else max_rss_gb
            )
        )
        cleanup = (
            "classified command as failed from child exit resource usage"
            if result.violation.scope == "process_rusage"
            else "terminated tracked process tree to prevent orphaned Molt subprocesses"
        )
        time_label = (
            "observed_at" if result.violation.scope == "process_rusage" else "killed_at"
        )
        print(
            "memory_guard: RSS limit exceeded; "
            f"{cleanup}: {time_label}={utc_timestamp()} "
            f"elapsed={elapsed_text(result.elapsed_s)} "
            f"{child_identity_text(result.child_process)} "
            f"pid={result.violation.pid} rss={result.violation.rss_gb:.2f}GB "
            f"limit={limit_text(limit_gb)} scope={result.violation.scope} "
            f"command={result.violation.command}",
            file=stderr,
        )
        print(
            "memory_guard: next action: inspect child logs and allocations for "
            "runaway work; lower parallelism/input size, or if expected raise the "
            "relevant *_MAX_PROCESS_RSS_GB/*_MAX_TOTAL_RSS_GB limit.",
            file=stderr,
        )
    if result.timed_out:
        print(
            "memory_guard: timeout after "
            f"{0.0 if timeout_s is None else timeout_s:.2f}s; "
            "terminated tracked process tree to prevent orphaned Molt "
            f"subprocesses: killed_at={utc_timestamp()} "
            f"elapsed={elapsed_text(result.elapsed_s)} "
            f"{child_identity_text(result.child_process)}",
            file=stderr,
        )
        print(
            "memory_guard: next action: inspect child logs for a hang or oversized "
            "workload; raise --timeout only for intentional long-running work.",
            file=stderr,
        )
    if result.orphaned_process_groups:
        pgids = ",".join(str(pgid) for pgid in result.orphaned_process_groups)
        print(
            "memory_guard: orphaned child processes detected after command exit; "
            "terminated tracked process groups to prevent accumulation: "
            f"killed_at={utc_timestamp()} elapsed={elapsed_text(result.elapsed_s)} "
            f"pgids={pgids} reason=direct child exited while descendants were "
            "still live",
            file=stderr,
        )
        print(
            "memory_guard: next action: inspect child process lifecycle and logs; "
            "make helpers shut down explicitly, or run intentional warm daemons "
            "inside a suite-level sentinel that drains at scope exit.",
            file=stderr,
        )
    exit_signal = signal_payload(result.returncode)
    if result.guard_signal is not None:
        guard_signal_payload = signal_payload(128 + result.guard_signal)
        signame = (
            guard_signal_payload["name"]
            if guard_signal_payload is not None
            and guard_signal_payload["name"] is not None
            else f"signal {result.guard_signal}"
        )
        print(
            "memory_guard: guard parent received "
            f"{signame}; summary written after terminating the tracked child tree: "
            f"observed_at={utc_timestamp()} elapsed={elapsed_text(result.elapsed_s)} "
            f"{child_identity_text(result.child_process)}",
            file=stderr,
        )
        print(
            (
                "memory_guard: next action: inspect the parent host/control-plane "
                "signal source and child logs; the RSS limit incident remains "
                "the primary classification."
                if result.violation is not None
                else (
                    "memory_guard: next action: inspect the parent host/control-plane "
                    "signal source and child logs; the timeout incident remains "
                    "the primary classification."
                    if result.timed_out
                    else "memory_guard: next action: inspect the parent "
                    "host/control-plane signal source and child logs; this was "
                    "not classified as an RSS limit trip."
                )
            ),
            file=stderr,
        )
        exit_signal = None
    if exit_signal is not None and result.violation is None and not result.timed_out:
        signame = exit_signal["name"] or f"signal {exit_signal['signal']}"
        print(
            "memory_guard: command exited with "
            f"{signame} status ({result.returncode}); no RSS violation observed: "
            f"observed_at={utc_timestamp()} elapsed={elapsed_text(result.elapsed_s)}",
            file=stderr,
        )
        print(
            "memory_guard: next action: inspect child stderr/logs or host signal "
            "source, including the direct-child RLIMIT_RSS backstop; "
            "the guard did not classify this as an RSS limit trip.",
            file=stderr,
        )
    if result.cargo_incremental_quarantine is not None:
        print(
            _cargo_incremental_quarantine_message(result.cargo_incremental_quarantine),
            file=stderr,
        )
        if result.cargo_incremental_quarantine.errors:
            print(
                "memory_guard: cargo incremental quarantine errors: "
                f"{'; '.join(result.cargo_incremental_quarantine.errors)}",
                file=stderr,
            )
            print(
                "memory_guard: next action: run `molt clean --apply "
                "--kill-processes` if stale Cargo state still blocks rebuilds.",
                file=stderr,
            )
    if repro_payload is not None:
        from tools.memory_guard_core.repro_context import repro_context_line

        print(
            f"memory_guard: repro context: {repro_context_line(repro_payload)}",
            file=stderr,
        )
