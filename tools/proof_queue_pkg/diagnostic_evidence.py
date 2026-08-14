"""Read-only proof log, guard summary, and live-process evidence."""

from __future__ import annotations

import json
import re
import sqlite3
import time
from pathlib import Path
from typing import Mapping

from tools.proof_queue_pkg import custody, state
from tools.proof_queue_pkg.diagnostic_model import _diagnostic, _format_duration

DIAGNOSTIC_LOG_TAIL_BYTES = 256 * 1024

RUNNING_CHILD_MISSING_STALE_LOG_SECONDS = 180.0

RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS = 60.0

PYTEST_PROGRESS_LINE_RE = re.compile(r"^[.FEfsxX]+(?:\s+\[\s*\d+%\])?$")


def _last_nonempty_log_line(path: Path) -> str | None:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - 65536))
            text = handle.read().decode("utf-8", errors="replace")
    except OSError:
        return None
    for line in reversed(text.splitlines()):
        stripped = line.strip()
        if stripped:
            return state._shorten(stripped)
    return None


def _first_log_line_containing(log_tail: str, needle: str) -> str | None:
    for line in log_tail.splitlines():
        if needle in line:
            return state._shorten(line)
    return None


def _read_log_tail(path: Path, *, limit: int = DIAGNOSTIC_LOG_TAIL_BYTES) -> str:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - limit))
            return handle.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


def _read_json_object(path: Path) -> dict[str, object]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return raw if isinstance(raw, dict) else {}


def _log_age_seconds(path: Path) -> float | None:
    try:
        return max(0.0, time.time() - path.stat().st_mtime)
    except OSError:
        return None


def _is_guard_command(command: object) -> bool:
    if isinstance(command, str):
        return "memory_guard.py" in command.replace("\\", "/")
    if not isinstance(command, list):
        return False
    return any(
        isinstance(part, str)
        and part.replace("\\", "/").endswith("tools/memory_guard.py")
        for part in command
    )


def _row_command_mentions_pytest(row: sqlite3.Row) -> bool:
    try:
        command = json.loads(row["command_json"])
    except (TypeError, json.JSONDecodeError):
        return False
    if not isinstance(command, list):
        return False
    return any(
        isinstance(part, str)
        and part.replace("\\", "/").lower().rsplit("/", 1)[-1]
        in {"pytest", "pytest.exe"}
        for part in command
    )


def _pytest_sections_from_summary(summary_json: object) -> list[dict[str, object]]:
    if not summary_json:
        return []
    summary = _read_json_object(Path(str(summary_json)))
    candidates: list[object] = [summary.get("pytest")]
    repro = summary.get("repro")
    if isinstance(repro, dict):
        candidates.append(repro.get("pytest"))
    return [item for item in candidates if isinstance(item, dict)]


def _pytest_current_status_line(summary_json: object) -> str | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        if isinstance(current_test_file, dict):
            payload = current_test_file.get("payload")
            if isinstance(payload, dict):
                nodeid = payload.get("nodeid")
                if isinstance(nodeid, str) and nodeid.strip():
                    phase = payload.get("phase")
                    line = f"  pytest_current={nodeid.strip()}"
                    if isinstance(phase, str) and phase.strip():
                        line += f" phase={phase.strip()}"
                    return line
            path = current_test_file.get("path")
            if current_test_file.get("missing") is True and isinstance(path, str):
                return f"  pytest_current=missing path={path}"
            error = current_test_file.get("error")
            if isinstance(error, str) and error.strip():
                return f"  pytest_current=unreadable error={state._shorten(error.strip(), 120)}"
        current = pytest_section.get("current_test")
        if isinstance(current, str) and current.strip():
            return f"  pytest_current={current.strip()}"
    return None


def _pytest_missing_current_test_file_evidence(summary_json: object) -> str | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        if not isinstance(current_test_file, dict):
            continue
        if current_test_file.get("missing") is not True:
            continue
        path = current_test_file.get("path")
        if isinstance(path, str) and path.strip():
            return f"pytest_current_test_file_missing path={path.strip()}"
        return "pytest_current_test_file_missing"
    return None


def _pytest_progress_failure_counts(line: str | None) -> tuple[int, int] | None:
    if line is None:
        return None
    progress = line.strip()
    if PYTEST_PROGRESS_LINE_RE.fullmatch(progress) is None:
        return None
    failures = progress.count("F")
    errors = progress.count("E")
    if failures == 0 and errors == 0:
        return None
    return failures, errors


def _summary_child_process(summary_json: object) -> dict[str, object] | None:
    if not summary_json:
        return None
    summary = _read_json_object(Path(str(summary_json)))
    child = summary.get("child_process")
    return child if isinstance(child, dict) else None


def _summary_host_platform(summary_json: object) -> str | None:
    if not summary_json:
        return None
    summary = _read_json_object(Path(str(summary_json)))
    repro = summary.get("repro")
    if not isinstance(repro, dict):
        return None
    host = repro.get("host")
    if not isinstance(host, dict):
        return None
    platform = host.get("platform")
    return platform if isinstance(platform, str) and platform.strip() else None


def _summary_payload_host_platform(summary: Mapping[str, object]) -> str | None:
    repro = summary.get("repro")
    if not isinstance(repro, dict):
        return None
    host = repro.get("host")
    if not isinstance(host, dict):
        return None
    platform = host.get("platform")
    return platform if isinstance(platform, str) and platform.strip() else None


def _memory_guard_child_runner_evidence(summary_json: object) -> str | None:
    child = _summary_child_process(summary_json)
    if child is None:
        return None
    command = child.get("command")
    if not _is_guard_command(command):
        return None
    label = (
        "windows_memory_guard_child_runner"
        if _summary_host_platform(summary_json) == "win32"
        else "memory_guard_child_process"
    )
    pid = child.get("pid")
    if isinstance(pid, int) and pid > 0:
        return f"child_process={label} pid={pid}"
    return f"child_process={label}"


def _memory_guard_child_runner_status_line(summary_json: object) -> str | None:
    evidence = _memory_guard_child_runner_evidence(summary_json)
    if evidence is None:
        return None
    return f"  guard_child={evidence}"


def _memory_guard_child_descendant_status_line(summary_json: object) -> str | None:
    child = _summary_child_process(summary_json)
    if child is None:
        return None
    child_pid = child.get("pid")
    if not isinstance(child_pid, int) or child_pid <= 0:
        return None
    if not _is_guard_command(child.get("command")):
        return None
    try:
        from tools import memory_guard

        samples = memory_guard.sample_processes()
        descendants = memory_guard.descendant_pids(samples, child_pid)
    except Exception:
        return None
    if not descendants:
        return None
    line = f"  guard_descendants={len(descendants)}"
    sample_evidence = _descendant_sample_evidence(samples, descendants)
    if sample_evidence is not None:
        line += f" {sample_evidence}"
    return line


def _descendant_sample_evidence(
    samples: Mapping[int, object],
    descendants: set[int],
    *,
    limit: int = 3,
) -> str | None:
    snippets: list[str] = []
    for pid in sorted(descendants):
        sample = samples.get(pid)
        command = getattr(sample, "command", None)
        if not isinstance(command, str) or not command.strip():
            continue
        if _low_signal_descendant_command(command):
            continue
        snippets.append(f"{pid}:{state._shorten(command, 96)}")
        if len(snippets) >= limit:
            break
    if not snippets:
        return None
    suffix = (
        ""
        if len(descendants) <= len(snippets)
        else f" +{len(descendants) - len(snippets)} more"
    )
    return "descendant_samples=" + "; ".join(snippets) + suffix


def _low_signal_descendant_command(command: str) -> bool:
    normalized = command.replace("\\", "/").strip().strip('"').casefold()
    return normalized.endswith("/conhost.exe") or normalized == "conhost.exe"


def _running_pytest_failures_observed_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    if not _row_command_mentions_pytest(row):
        return None
    log_path = Path(row["log_path"])
    last_log_line = _last_nonempty_log_line(log_path)
    counts = _pytest_progress_failure_counts(last_log_line)
    if counts is None:
        return None
    failures, errors = counts
    evidence_parts = [
        f"last_pytest_progress={last_log_line}",
        f"failures={failures}",
        f"errors={errors}",
    ]
    log_age_s = _log_age_seconds(log_path)
    if log_age_s is not None:
        evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    child_runner = _memory_guard_child_runner_evidence(row["summary_json"])
    if child_runner is not None:
        evidence_parts.append(child_runner)
    return _diagnostic(
        signal_id="running-pytest-failures-observed",
        severity="warning",
        summary=(
            "Running pytest proof has already emitted failure/error progress markers."
        ),
        evidence=" ".join(evidence_parts),
        next_action=(
            "Keep the row running for the full pytest failure report; do not "
            "classify this as infra-only current-test opacity or interrupt via "
            "Codex stdin."
        ),
        scopes=("tools/proof_queue.py",),
        artifacts=(str(row["log_path"]),),
    )


def _running_pytest_current_test_missing_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    if not _row_command_mentions_pytest(row):
        return None
    log_path = Path(row["log_path"])
    log_age_s = _log_age_seconds(log_path)
    if (
        log_age_s is None
        or log_age_s < RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
    ):
        return None
    evidence = _pytest_missing_current_test_file_evidence(row["summary_json"])
    if evidence is None:
        return None
    evidence_parts: list[str] = []
    child_runner = _memory_guard_child_runner_evidence(row["summary_json"])
    if child_runner is not None:
        evidence_parts.append(child_runner)
    evidence_parts.append(evidence)
    evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    last_log_line = _last_nonempty_log_line(log_path)
    pytest_progress = (
        last_log_line
        if last_log_line is not None
        and PYTEST_PROGRESS_LINE_RE.fullmatch(last_log_line.strip()) is not None
        else None
    )
    if pytest_progress is not None:
        evidence_parts.append(f"last_pytest_progress={pytest_progress}")
        summary = (
            "Running pytest proof emitted progress output but has no current-test "
            "marker."
        )
        next_action = (
            "Treat this as current-test custody opacity after pytest started, "
            "not collection/startup opacity. Inspect pytest guard plugin/env "
            "wiring once, then rerun with a focused selector if the row does not "
            "finish; do not interrupt via Codex stdin."
        )
    else:
        if last_log_line:
            evidence_parts.append(f"last_log={last_log_line}")
        summary = (
            "Running pytest proof has no current-test marker while its queue log "
            "is quiet."
        )
        next_action = (
            "Treat this as pre-test or collection/startup opacity. Inspect the "
            "pytest startup path, uv/cache contention, or Windows memory-guard "
            "child-runner descendant once, then rerun with a first-failure or "
            "collection-focused proof; do not interrupt via Codex stdin."
        )
    evidence = " ".join(evidence_parts)
    return _diagnostic(
        signal_id="running-pytest-current-test-missing",
        severity="infra",
        summary=summary,
        evidence=evidence,
        next_action=next_action,
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )


def _running_child_missing_diagnostic(row: sqlite3.Row) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    log_age_s = _log_age_seconds(Path(row["log_path"]))
    if log_age_s is None or log_age_s < RUNNING_CHILD_MISSING_STALE_LOG_SECONDS:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    child = summary.get("child_process")
    if not isinstance(child, dict):
        if summary.get("status") == "running" and summary.get("returncode") is None:
            evidence = (
                f"summary_json={row['summary_json']} summary_status=running "
                f"child_process=null returncode=null "
                f"last_log_age={_format_duration(log_age_s)}"
            )
            recorded_at = summary.get("recorded_at")
            if isinstance(recorded_at, str) and recorded_at.strip():
                evidence += f" recorded_at={recorded_at.strip()}"
            return _diagnostic(
                signal_id="running-proof-launch-summary-stale",
                severity="infra",
                summary=(
                    "Running proof row still has only the guard launch summary "
                    "after its log went stale."
                ),
                evidence=evidence,
                next_action=(
                    "Treat the row as custody-incomplete evidence. Inspect the "
                    "log and guard summary, then use `prune-stale` or rerun "
                    "through the same queue lane; do not interrupt via Codex stdin."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        return None
    child_pid = child.get("pid")
    if not isinstance(child_pid, int) or child_pid <= 0:
        return None
    child_is_guard_command = _is_guard_command(child.get("command"))
    if not child_is_guard_command:
        return None
    host_platform = _summary_payload_host_platform(summary)
    child_runner_label = (
        "windows_memory_guard_child_runner"
        if host_platform == "win32"
        else "memory_guard_child_process"
    )
    if not custody._pid_alive(child_pid):
        evidence = (
            f"child_pid={child_pid} last_log_age={_format_duration(log_age_s)} "
            f"summary_json={row['summary_json']}"
        )
        if host_platform == "win32":
            evidence += f" child_process={child_runner_label}"
            return _diagnostic(
                signal_id="running-proof-windows-child-runner-missing",
                severity="infra",
                summary=(
                    "Running proof row's Windows memory-guard child runner is "
                    "no longer live while the queue-owned guard still owns the row."
                ),
                evidence=evidence,
                next_action=(
                    "Treat this as Windows memory-guard custody opacity, not a "
                    "terminal stale signal while the queue-owned guard is live. "
                    "Wait for the guard's final summary, or let prune-stale "
                    "reclaim it only after the guard exits or the running-age "
                    "ceiling is exceeded."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        summary_text = (
            "Running proof row's nested memory guard child is no longer live."
        )
    else:
        try:
            from tools import memory_guard

            samples = memory_guard.sample_processes()
            descendants = memory_guard.descendant_pids(samples, child_pid)
        except Exception as exc:  # pragma: no cover - sampler failure is host-specific.
            return _diagnostic(
                signal_id="running-proof-custody-sampler-failed",
                severity="infra",
                summary="Running proof row could not be inspected by the process sampler.",
                evidence=(
                    f"child_pid={child_pid} sampler_error={type(exc).__name__}: {exc} "
                    f"summary_json={row['summary_json']}"
                ),
                next_action=(
                    "Inspect the memory-guard summary and process custody manually, "
                    "then fix the sampler if this host shape repeats."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
            )
        if descendants:
            sample_evidence = _descendant_sample_evidence(samples, descendants)
            evidence = (
                f"child_pid={child_pid} last_log_age={_format_duration(log_age_s)} "
                f"descendants={len(descendants)} summary_json={row['summary_json']}"
            )
            if sample_evidence is not None:
                evidence += f" {sample_evidence}"
            return _diagnostic(
                signal_id="running-proof-log-stale-live-child",
                severity="infra",
                summary=(
                    "Running proof row has a stale log, but its nested memory "
                    "guard still owns live work descendants."
                ),
                evidence=evidence,
                next_action=(
                    "Do not prune or interrupt this row from the diagnostic alone. "
                    "Inspect the child command or add one queue note if the row is "
                    "compile-dominated, then let the owner timeout/finish or rerun "
                    "with a better-shaped proof."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        evidence = (
            f"child_pid={child_pid} last_log_age={_format_duration(log_age_s)} "
            f"descendants=0 summary_json={row['summary_json']}"
        )
        summary_text = (
            "Running proof row has a live nested memory guard but no visible "
            "work child beneath it."
        )
    return _diagnostic(
        signal_id="running-proof-child-missing",
        severity="infra",
        summary=summary_text,
        evidence=evidence,
        next_action=(
            "Treat the row as custody-incomplete evidence. Inspect the log and "
            "memory-guard summary, then use `prune-stale` or rerun through the "
            "same queue lane; do not interrupt via Codex stdin."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )


def _finished_incomplete_memory_guard_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] in state.RUNNING:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    summary_status = summary.get("status")
    if summary_status not in {"running", "child_running"}:
        return None
    if summary.get("returncode") is not None:
        return None
    evidence_parts = [
        f"row_status={row['status']}",
        f"row_returncode={row['returncode']}",
        f"summary_status={summary_status}",
        "summary_returncode=null",
    ]
    elapsed_s = row["elapsed_s"]
    if isinstance(elapsed_s, (int, float)):
        evidence_parts.append(f"row_elapsed={_format_duration(float(elapsed_s))}")
    limits = summary.get("limits")
    repro = summary.get("repro")
    if not isinstance(limits, dict) and isinstance(repro, dict):
        limits = repro.get("limits")
    if isinstance(limits, dict):
        timeout_s = limits.get("timeout_s")
        if isinstance(timeout_s, (int, float)):
            evidence_parts.append(
                f"configured_timeout={_format_duration(float(timeout_s))}"
            )
    child = summary.get("child_process")
    if isinstance(child, dict):
        command = child.get("command")
        child_runner = (
            _memory_guard_child_runner_evidence(row["summary_json"])
            if _is_guard_command(command)
            else None
        )
        if child_runner is not None:
            evidence_parts.append(child_runner)
        else:
            child_pid = child.get("pid")
            if isinstance(child_pid, int) and child_pid > 0:
                evidence_parts.append(f"child_pid={child_pid}")
    recorded_at = summary.get("recorded_at")
    if isinstance(recorded_at, str) and recorded_at.strip():
        evidence_parts.append(f"recorded_at={recorded_at.strip()}")
    log_age_s = _log_age_seconds(Path(row["log_path"]))
    if log_age_s is not None:
        evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    last_log_line = _last_nonempty_log_line(Path(row["log_path"]))
    if last_log_line:
        evidence_parts.append(f"last_log={last_log_line}")
    evidence_parts.append(f"summary_json={row['summary_json']}")
    evidence = " ".join(evidence_parts)
    return _diagnostic(
        signal_id="memory-guard-summary-incomplete",
        severity="infra",
        summary=(
            "Terminal proof row has only a non-final memory-guard running summary."
        ),
        evidence=evidence,
        next_action=(
            "Treat this row as queue-custody incomplete evidence, not product "
            "proof. Inspect the queue log and summary once, then rerun through "
            "the same queue lane or fix memory_guard final-summary lifecycle if "
            "the pattern repeats."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )


def _finished_worker_exit_without_summary_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] in state.RUNNING:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    if summary.get("status") != "guard_worker_exited_without_final_summary":
        return None
    evidence_parts = [
        f"row_status={row['status']}",
        f"row_returncode={row['returncode']}",
    ]
    worker_returncode = summary.get("worker_returncode")
    if worker_returncode is not None:
        evidence_parts.append(f"worker_returncode={worker_returncode}")
    previous_status = None
    incident = summary.get("incident")
    if isinstance(incident, dict):
        raw_previous_status = incident.get("previous_status")
        if isinstance(raw_previous_status, str) and raw_previous_status.strip():
            previous_status = raw_previous_status.strip()
            evidence_parts.append(f"previous_status={previous_status}")
    signal_payload = summary.get("worker_exit_signal") or summary.get("exit_signal")
    if isinstance(signal_payload, dict):
        signal_name = signal_payload.get("name")
        signal_number = signal_payload.get("signal")
        if isinstance(signal_name, str) and signal_name.strip():
            evidence_parts.append(f"worker_signal={signal_name.strip()}")
        elif isinstance(signal_number, int):
            evidence_parts.append(f"worker_signal={signal_number}")
    child = summary.get("child_process")
    if isinstance(child, dict):
        command = child.get("command")
        child_runner = (
            _memory_guard_child_runner_evidence(row["summary_json"])
            if _is_guard_command(command)
            else None
        )
        if child_runner is not None:
            evidence_parts.append(child_runner)
        else:
            child_pid = child.get("pid")
            if isinstance(child_pid, int) and child_pid > 0:
                evidence_parts.append(f"child_pid={child_pid}")
    recorded_at = summary.get("recorded_at")
    if isinstance(recorded_at, str) and recorded_at.strip():
        evidence_parts.append(f"recorded_at={recorded_at.strip()}")
    last_log_line = _last_nonempty_log_line(Path(row["log_path"]))
    if last_log_line:
        evidence_parts.append(f"last_log={last_log_line}")
    evidence_parts.append(f"summary_json={row['summary_json']}")
    return _diagnostic(
        signal_id="memory-guard-worker-exit-without-final-summary",
        severity="infra",
        summary=(
            "Terminal proof row was preserved by the memory_guard wrapper after "
            "the internal guard worker exited before writing the final summary."
        ),
        evidence=" ".join(evidence_parts),
        next_action=(
            "Treat this row as guard-custody failure evidence, not product proof. "
            "Inspect the worker signal/source and child logs once, then rerun "
            "through the same queue lane after the guard lifecycle is fixed."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )


def _pytest_timeout_context(summary_json: object) -> tuple[str, str | None] | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        payload: object = None
        if isinstance(current_test_file, dict):
            payload = current_test_file.get("payload")
        if not isinstance(payload, dict):
            continue
        nodeid = payload.get("nodeid")
        if not isinstance(nodeid, str) or not nodeid.strip():
            continue
        phase = payload.get("phase")
        phase_text = phase.strip() if isinstance(phase, str) and phase.strip() else None
        return nodeid.strip(), phase_text
    return None
