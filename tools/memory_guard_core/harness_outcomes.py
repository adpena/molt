from __future__ import annotations

import datetime as dt
from collections.abc import Sequence
from typing import Protocol

from tools import memory_guard


class HarnessLimitsView(Protocol):
    max_process_rss_gb: float
    max_total_rss_gb: float


def normalize_prefix(prefix: str) -> str:
    return prefix.strip().upper().rstrip("_")


def utc_timestamp() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z")
    )


def elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def limit_text(limit_gb: float | None) -> str:
    return "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"


def rss_limit_hint(prefix: str) -> str:
    normalized = normalize_prefix(prefix) or "MOLT"
    if normalized == "MOLT":
        return "MOLT_MAX_PROCESS_RSS_GB/MOLT_MAX_TOTAL_RSS_GB"
    return (
        f"{normalized}_MAX_PROCESS_RSS_GB/{normalized}_MAX_TOTAL_RSS_GB "
        "or the parent MOLT_MAX_* RSS limits"
    )


def timeout_hint(prefix: str) -> str:
    normalized = normalize_prefix(prefix) or "MOLT"
    return f"{normalized}_TIMEOUT_SEC or MOLT_TEST_PROCESS_TIMEOUT_SEC"


def guard_stderr_message(
    violation: memory_guard.RssViolation | None,
    limits: HarnessLimitsView,
    effective_limits: memory_guard.ResolvedMemoryLimits | None = None,
    *,
    prefix: str,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if violation is None:
        return ""
    limit_gb = (
        (
            effective_limits.max_total_rss_gb
            if effective_limits is not None
            else limits.max_total_rss_gb
        )
        if violation.scope == "process_tree"
        else (
            effective_limits.max_process_rss_gb
            if effective_limits is not None
            else limits.max_process_rss_gb
        )
    )
    cleanup = (
        "classified the command as failed from child exit resource usage"
        if violation.scope == "process_rusage"
        else "terminated the tracked process tree to prevent orphaned Molt subprocesses"
    )
    time_label = "observed_at" if violation.scope == "process_rusage" else "killed_at"
    return (
        "memory_guard: RSS limit exceeded; "
        f"{cleanup}: {time_label}={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"pid={violation.pid} rss={violation.rss_gb:.2f}GB "
        f"limit={limit_text(limit_gb)} scope={violation.scope} "
        f"command={violation.command}\n"
        "memory_guard: next action: inspect child logs and allocations for runaway "
        "work; lower parallelism/input size, or if this workload is expected raise "
        f"{rss_limit_hint(prefix)} within repo policy.\n"
    )


def guard_timeout_message(
    *,
    prefix: str,
    timeout: float | None,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    timeout_text = "unknown" if timeout is None else f"{timeout:.2f}s"
    return (
        "memory_guard: timeout; terminated the tracked process tree to prevent "
        "orphaned Molt subprocesses: "
        f"killed_at={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"timeout={timeout_text}\n"
        "memory_guard: next action: inspect child logs for a hang or oversized "
        f"workload; if intentional raise {timeout_hint(prefix)} for this guard "
        "family.\n"
    )


def guard_exit_signal_message(
    returncode: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
) -> str:
    payload = memory_guard.exit_signal_payload(returncode)
    if payload is None:
        return ""
    signame = payload["name"] or f"signal {payload['signal']}"
    return (
        "memory_guard: command exited with "
        f"{signame} status ({returncode}); no RSS violation observed: "
        f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect child stderr/logs or host signal "
        "source, including the direct-child RLIMIT_RSS backstop; if "
        "host memory pressure was involved, rerun with guard samples and lower "
        "parallelism.\n"
    )


def guard_parent_signal_message(
    guard_signal: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
    primary_reason: str | None = None,
) -> str:
    payload = memory_guard.exit_signal_payload(128 + guard_signal)
    signame = (
        payload["name"]
        if payload is not None and payload["name"] is not None
        else f"signal {guard_signal}"
    )
    if primary_reason is None:
        return (
            "memory_guard: guard parent received "
            f"{signame}; terminated tracked process tree before exiting: "
            f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
            "memory_guard: next action: inspect the parent host/control-plane "
            "signal source and child logs; the guard parent received the signal "
            "and wrote this custody record before exiting.\n"
        )
    return (
        "memory_guard: guard parent also received "
        f"{signame} while primary incident remained {primary_reason}: "
        f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect the parent host/control-plane "
        "signal source and child logs; preserve the primary incident "
        "classification when triaging this run.\n"
    )


def guard_orphan_cleanup_message(
    process_groups: Sequence[int],
    *,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if not process_groups:
        return ""
    pgids = ",".join(str(pgid) for pgid in process_groups)
    return (
        "memory_guard: orphaned child processes detected after command exit; "
        "terminated tracked process groups to prevent accumulation: "
        f"killed_at={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"pgids={pgids} reason=direct child exited while descendants were still "
        "live\n"
        "memory_guard: next action: inspect child process lifecycle and logs; "
        "make helpers shut down explicitly, or run intentional warm daemons inside "
        "a suite-level sentinel that drains at scope exit.\n"
    )


def rss_record_payload(
    record: memory_guard.RssViolation | None,
) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def guarded_command_status(
    *,
    returncode: int,
    violation: memory_guard.RssViolation | None,
    timed_out: bool,
    orphaned_process_groups: Sequence[int],
    guard_signal: int | None = None,
) -> str:
    if violation is not None:
        return "rss_limit_exceeded"
    if timed_out:
        return "timeout"
    if guard_signal is not None:
        return "guard_interrupted"
    if memory_guard.exit_signal_payload(returncode) is not None:
        return "signal_exit"
    if returncode != 0:
        return "failed"
    if orphaned_process_groups:
        return "pass_with_orphan_cleanup"
    return "pass"
