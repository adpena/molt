import datetime as dt
import json
import re
import subprocess
from pathlib import Path
from typing import Any

from bench_friends_env import _emit_progress
from bench_friends_types import MAX_FAILURE_MESSAGE_CHARS, PhaseResult

import harness_memory_guard
import bench as bench_tool


def _parse_stdout_json(stdout: str) -> tuple[Any | None, str | None]:
    text = stdout.strip()
    if not text:
        return None, "stdout was empty"
    try:
        return json.loads(text), None
    except json.JSONDecodeError as exc:
        return None, f"stdout was not valid JSON: {exc}"


def _metric_slug(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9_]+", "_", value).strip("_").lower()
    return slug or "metric"


def _as_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


def _extract_structured_elapsed(payload: Any) -> dict[str, float]:
    if not isinstance(payload, dict):
        return {}

    metrics: dict[str, float] = {}
    workloads = payload.get("workloads")
    if isinstance(workloads, dict):
        for workload_name, entry in workloads.items():
            if not isinstance(entry, dict):
                continue
            elapsed = _as_float(entry.get("elapsed_s"))
            if elapsed is not None:
                metrics[_metric_slug(str(workload_name))] = elapsed

    results = payload.get("results")
    if isinstance(results, list):
        for idx, entry in enumerate(results, start=1):
            if not isinstance(entry, dict):
                continue
            elapsed = _as_float(entry.get("elapsed_s"))
            if elapsed is None:
                continue
            label = entry.get("benchmark") or entry.get("workload") or f"result_{idx}"
            metrics[_metric_slug(str(label))] = elapsed

    top_elapsed = _as_float(payload.get("elapsed_s"))
    if top_elapsed is not None:
        metrics.setdefault("total", top_elapsed)
    total_elapsed = _as_float(payload.get("total_elapsed_s"))
    if total_elapsed is not None:
        metrics["total"] = total_elapsed
    return metrics


def _rss_record_payload(record: Any | None) -> dict[str, Any] | None:
    if record is None:
        return None
    return {
        "pid": getattr(record, "pid", None),
        "rss_kb": getattr(record, "rss_kb", None),
        "rss_gb": getattr(record, "rss_gb", None),
        "command": getattr(record, "command", None),
        "scope": getattr(record, "scope", None),
    }


def _guard_status(
    *,
    returncode: int,
    violation: Any | None,
    timed_out: bool,
    orphaned_process_groups: list[int],
) -> str:
    if violation is not None:
        return "rss_limit_exceeded"
    if timed_out:
        return "timeout"
    if harness_memory_guard.memory_guard.exit_signal_payload(returncode) is not None:
        return "signal_exit"
    if returncode != 0:
        return "failed"
    if orphaned_process_groups:
        return "pass_with_orphan_cleanup"
    return "pass"


def _guarded_phase_diagnostics(
    res: subprocess.CompletedProcess[str],
) -> dict[str, Any]:
    orphaned_process_groups = [
        int(pgid) for pgid in getattr(res, "orphaned_process_groups", ()) or ()
    ]
    violation = getattr(res, "violation", None)
    timed_out = bool(getattr(res, "timed_out", False))
    limit_at_violation = getattr(res, "limit_at_violation", None)
    cargo_quarantine = getattr(res, "cargo_incremental_quarantine", None)
    return {
        "guard_status": _guard_status(
            returncode=res.returncode,
            violation=violation,
            timed_out=timed_out,
            orphaned_process_groups=orphaned_process_groups,
        ),
        "guard_violation": _rss_record_payload(violation),
        "guard_limit_at_violation": (
            None
            if limit_at_violation is None
            else harness_memory_guard.memory_guard.memory_limits_payload(
                limit_at_violation
            )
        ),
        "guard_orphaned_process_groups": orphaned_process_groups,
        "guard_exit_signal": (
            None
            if violation is not None or timed_out
            else harness_memory_guard.memory_guard.exit_signal_payload(res.returncode)
        ),
        "guard_cargo_incremental_quarantine": (
            None
            if cargo_quarantine is None
            else harness_memory_guard.memory_guard._cargo_incremental_quarantine_payload(
                cargo_quarantine
            )
        ),
    }


def _molt_failure_reason_suffix(payload: dict[str, Any] | None) -> str:
    if not payload:
        return ""
    detail = payload.get("detail")
    detail_text = f" ({detail})" if detail else ""
    return f": {payload.get('status', 'failed')}{detail_text}"


def _bounded_failure_text(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value)
    if not text:
        return None
    if len(text) <= MAX_FAILURE_MESSAGE_CHARS:
        return text
    return (
        f"... <truncated to last {MAX_FAILURE_MESSAGE_CHARS} chars>\n"
        f"{text[-MAX_FAILURE_MESSAGE_CHARS:]}"
    )


def _molt_failure_with_log_refs(
    payload: dict[str, Any],
    *,
    stdout_path: Path,
    stderr_path: Path,
) -> dict[str, Any]:
    enriched = dict(payload)
    enriched["message"] = _bounded_failure_text(enriched.get("message"))
    enriched["log_refs"] = [
        {"kind": "stdout", "path": str(stdout_path)},
        {"kind": "stderr", "path": str(stderr_path)},
    ]
    return enriched


def _run_command(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout_sec: int,
    stdout_path: Path,
    stderr_path: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
    parse_stdout_json: bool = False,
    molt_failure_phase: str | None = None,
    progress_label: str | None = None,
) -> PhaseResult:
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stderr_path.parent.mkdir(parents=True, exist_ok=True)
    if dry_run:
        if progress_label is not None:
            _emit_progress(
                f"start {progress_label} dry_run=true timeout_s={timeout_sec} "
                f"stdout={stdout_path} stderr={stderr_path}"
            )
        stdout_path.write_text(
            f"[dry-run] cwd={cwd}\n$ {' '.join(cmd)}\n", encoding="utf-8"
        )
        stderr_path.write_text("", encoding="utf-8")
        if progress_label is not None:
            _emit_progress(f"finish {progress_label} status=ok elapsed_s=0.000")
        return PhaseResult(
            cmd=cmd,
            returncode=0,
            elapsed_s=0.0,
            timed_out=False,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
        )

    start = dt.datetime.now(dt.timezone.utc)
    if progress_label is not None:
        _emit_progress(
            f"start {progress_label} timeout_s={timeout_sec} argv_count={len(cmd)} "
            f"stdout={stdout_path} stderr={stderr_path}"
        )
    timed_out = False
    guard_elapsed_s: float | None = None
    diagnostics: dict[str, Any] = {}
    res: subprocess.CompletedProcess[str] | None = None
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            cwd=str(cwd),
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            limits=limits,
        )
        guard_elapsed_s = res.elapsed_s
        timed_out = (
            res.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        )
        rc = -9 if timed_out else res.returncode
        stdout = res.stdout or ""
        stderr = res.stderr or ""
        diagnostics = _guarded_phase_diagnostics(res)
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        rc = -9
        stdout = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
        stderr = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
        stderr = f"{stderr}\n[timeout] command exceeded {timeout_sec}s\n"
        diagnostics = {
            "guard_status": "timeout",
            "guard_violation": None,
            "guard_limit_at_violation": None,
            "guard_orphaned_process_groups": [],
            "guard_exit_signal": None,
            "guard_cargo_incremental_quarantine": None,
        }
    end = dt.datetime.now(dt.timezone.utc)
    elapsed = (
        guard_elapsed_s
        if guard_elapsed_s is not None
        else (end - start).total_seconds()
    )
    stdout_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    stdout_json = None
    stdout_json_error = None
    if parse_stdout_json:
        stdout_json, stdout_json_error = _parse_stdout_json(stdout)
    molt_failure = None
    if molt_failure_phase is not None and (
        rc != 0
        or timed_out
        or diagnostics.get("guard_violation") is not None
        or diagnostics.get("guard_orphaned_process_groups")
    ):
        failure = bench_tool.classify_molt_process_failure(
            phase=molt_failure_phase,
            returncode=rc,
            stdout=stdout,
            stderr=stderr,
            elapsed_s=elapsed,
            timed_out=timed_out,
            violation=getattr(res, "violation", None) if res is not None else None,
            orphaned_process_groups=tuple(
                int(pgid)
                for pgid in diagnostics.get("guard_orphaned_process_groups", []) or []
            ),
        )
        molt_failure = _molt_failure_with_log_refs(
            bench_tool.molt_failure_payload(failure),
            stdout_path=stdout_path,
            stderr_path=stderr_path,
        )
    phase_result = PhaseResult(
        cmd=cmd,
        returncode=rc,
        elapsed_s=elapsed,
        timed_out=timed_out,
        stdout_path=str(stdout_path),
        stderr_path=str(stderr_path),
        stdout_json=stdout_json,
        stdout_json_error=stdout_json_error,
        molt_failure=molt_failure,
        **diagnostics,
    )
    if progress_label is not None:
        status = "ok" if phase_result.ok else "failed"
        timeout_suffix = " timed_out=true" if timed_out else ""
        _emit_progress(
            f"finish {progress_label} status={status} rc={rc} "
            f"elapsed_s={elapsed:.3f}{timeout_suffix}"
        )
    return phase_result
