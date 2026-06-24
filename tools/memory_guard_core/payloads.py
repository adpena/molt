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
