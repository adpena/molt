"""Human-facing proof status and diagnosis-note rendering."""

from __future__ import annotations

import sqlite3
import time
from pathlib import Path

from tools.proof_queue_pkg.diagnostic_engine import _run_diagnostics
from tools.proof_queue_pkg.diagnostic_evidence import (
    _last_nonempty_log_line,
    _memory_guard_child_descendant_status_line,
    _memory_guard_child_runner_status_line,
    _pytest_current_status_line,
    _row_command_mentions_pytest,
)
from tools.proof_queue_pkg.diagnostic_model import (
    _diagnostic_artifacts,
    _format_diagnostic_summary,
    _format_duration,
)


def _active_log_status(row: sqlite3.Row) -> list[str]:
    path = Path(row["log_path"])
    try:
        stat = path.stat()
    except OSError:
        if row["status"] == "queued":
            return [f"  log={path} (queued; proof command not launched yet)"]
        if row["status"] == "dispatched":
            return [f"  log={path} (dispatched; waiting for detached runner)"]
        return [f"  log={path} (missing)"]
    age = _format_duration(max(0.0, time.time() - stat.st_mtime))
    lines = [f"  log={path}", f"  last_log_age={age}"]
    last = _last_nonempty_log_line(path)
    if last:
        lines[-1] = f"{lines[-1]} last={last}"
    pytest_line = (
        _pytest_current_status_line(row["summary_json"])
        if _row_command_mentions_pytest(row)
        else None
    )
    if pytest_line:
        lines.append(pytest_line)
    child_runner_line = _memory_guard_child_runner_status_line(row["summary_json"])
    if child_runner_line:
        lines.append(child_runner_line)
    child_descendant_line = _memory_guard_child_descendant_status_line(
        row["summary_json"]
    )
    if child_descendant_line:
        lines.append(child_descendant_line)
    return lines


def _print_status_diagnostics(row: sqlite3.Row) -> None:
    diagnostics = _run_diagnostics(row)
    diagnostic_summary = _format_diagnostic_summary(diagnostics)
    if diagnostic_summary:
        print(f"  diagnosis={diagnostic_summary}")
    artifacts = _diagnostic_artifacts(diagnostics)
    if artifacts:
        print(f"  artifacts={', '.join(artifacts)}")


def _diagnosis_note_body(row: sqlite3.Row, diagnostics: list[dict[str, object]]) -> str:
    if diagnostics:
        first = diagnostics[0]
        artifact_text = ""
        artifacts = _diagnostic_artifacts(diagnostics)
        if artifacts:
            artifact_text = " artifacts: " + ", ".join(artifacts)
        return (
            f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
            f"{first['signal_id']}: {first['summary']}{artifact_text} "
            f"next: {first['next_action']}"
        )
    return (
        f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
        "has no queue diagnostic signals."
    )
