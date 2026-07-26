from __future__ import annotations

from collections.abc import Sequence
from typing import Any

from tools.memory_guard_core.memory_limits import ResolvedMemoryLimits


def _rss_record_payload(record: Any | None) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def guarded_child_process_payload(
    child: Any | None,
) -> dict[str, object] | None:
    if child is None:
        return None
    return {
        "pid": child.pid,
        "pgid": child.pgid,
        "sid": child.sid,
        "command": list(child.command),
        "started_at": child.started_at,
    }


def termination_action_payload(
    action: Any,
) -> dict[str, object]:
    payload: dict[str, object] = {
        "target_kind": action.target_kind,
        "target_id": action.target_id,
        "signal": action.signal,
        "signal_name": action.signal_name,
        "result": action.result,
    }
    if action.error is not None:
        payload["error"] = action.error
    return payload


def termination_report_payload(
    report: Any,
) -> dict[str, object]:
    return {
        "reason": report.reason,
        "started_at": report.started_at,
        "completed_at": report.completed_at,
        "root_pid": report.root_pid,
        "root_pgid": report.root_pgid,
        "root_sid": report.root_sid,
        "grace_sec": report.grace_sec,
        "watched_pids": list(report.watched_pids),
        "protected_pgids": list(report.protected_pgids),
        "escaped_pids": list(report.escaped_pids),
        "remaining_pgids": list(report.remaining_pgids),
        "remaining_pids": list(report.remaining_pids),
        "actions": [termination_action_payload(action) for action in report.actions],
    }


def termination_reports_payload(
    reports: Sequence[Any],
) -> list[dict[str, object]]:
    return [termination_report_payload(report) for report in reports]


def _windows_system_resources_payload(resources: Any) -> dict[str, object]:
    return {
        "process_count": resources.process_count,
        "thread_count": resources.thread_count,
        "system_handle_count": resources.system_handle_count,
        "guard_handle_count": resources.guard_handle_count,
        "commit_total_bytes": resources.commit_total_bytes,
        "commit_limit_bytes": resources.commit_limit_bytes,
        "commit_peak_bytes": resources.commit_peak_bytes,
        "physical_total_bytes": resources.physical_total_bytes,
        "physical_available_bytes": resources.physical_available_bytes,
        "errors": list(resources.errors),
    }


def _windows_job_accounting_payload(accounting: Any) -> dict[str, object]:
    return {
        "total_processes": accounting.total_processes,
        "active_processes": accounting.active_processes,
        "total_terminated_processes": accounting.total_terminated_processes,
        "peak_job_commit_bytes": accounting.peak_job_commit_bytes,
    }


def windows_job_cleanup_payload(cleanup: Any | None) -> dict[str, object] | None:
    if cleanup is None:
        return None
    return {
        "completed": cleanup.completed,
        "terminated_remaining_processes": cleanup.terminated_remaining_processes,
        "elapsed_s": cleanup.elapsed_s,
        "before": _windows_job_accounting_payload(cleanup.before),
        "after": _windows_job_accounting_payload(cleanup.after),
        "system_before": _windows_system_resources_payload(cleanup.system_before),
        "system_after": _windows_system_resources_payload(cleanup.system_after),
    }


def memory_limits_payload(limits: ResolvedMemoryLimits) -> dict[str, object]:
    budget = limits.adaptive_budget
    return {
        "max_process_rss_gb": limits.max_process_rss_gb,
        "max_total_rss_gb": limits.max_total_rss_gb,
        "max_global_rss_gb": limits.max_global_rss_gb,
        "dynamic_process_rss": limits.dynamic_process_rss,
        "dynamic_total_rss": limits.dynamic_total_rss,
        "dynamic_global_rss": limits.dynamic_global_rss,
        "adaptive_budget": None
        if budget is None
        else {
            "source": budget.source,
            "reserve_gb": budget.reserve_gb,
            "physical_gb": budget.physical_gb,
            "available_gb": budget.available_gb,
            "accounted_rss_gb": budget.accounted_rss_gb,
        },
    }
