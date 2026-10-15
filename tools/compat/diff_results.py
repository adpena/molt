"""The one reader of molt_diff's per-test result log.

With ``MOLT_DIFF_RESULTS_JSONL`` set, ``tests/molt_diff.py`` appends one
``record_type: "test"`` row per test as it finishes, and one
``record_type: "backend"`` row per backend, from every worker process. The log
therefore survives a killed run. Consumers parse rows here and apply their own
policy to them.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from pathlib import Path
from typing import Any

from molt.exact_json import ExactJsonError, loads_exact
from tools.compat.item_outcome import ItemOutcome, ItemStatus

RESULTS_ENV = "MOLT_DIFF_RESULTS_JSONL"
_DETAIL_CHARS = 160


def load_rows(path: Path) -> tuple[Mapping[str, Any], ...]:
    """Every row of a result log, in write order; any malformed line fails."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"differential result log is unreadable: {exc}") from exc
    rows: list[Mapping[str, Any]] = []
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = loads_exact(line)
        except (ValueError, ExactJsonError) as exc:
            raise ValueError(
                f"differential result line {line_number} is invalid: {exc}"
            ) from exc
        if not isinstance(row, Mapping):
            raise ValueError(f"differential result line {line_number} is not an object")
        rows.append(row)
    return tuple(rows)


def _failure_reason(test: Mapping[str, Any], backends: list[Mapping[str, Any]]) -> str:
    tag = test.get("reason_tag")
    if tag == "xpass":
        return "xpass"
    if tag == "timeout" or any(row.get("timed_out") for row in backends):
        return "timeout"
    if tag in {"infrastructure_error", "guard_interrupted"}:
        return "infrastructure"
    if test.get("resolved_status") == "oom" or any(
        row.get("resource_failure") or row.get("rss_limit_exceeded") for row in backends
    ):
        return "resources"
    if any(row.get("build_failed") for row in backends):
        return "build"
    return "mismatch"


def outcomes(rows: Iterable[Mapping[str, Any]]) -> dict[str, ItemOutcome]:
    """Each finished test's outcome, keyed by its repo-relative path."""
    tests: dict[str, Mapping[str, Any]] = {}
    backends: dict[str, list[Mapping[str, Any]]] = {}
    for row in rows:
        path = row.get("file")
        if not isinstance(path, str):
            raise ValueError("differential result row has no file")
        if row.get("record_type") == "backend":
            backends.setdefault(path, []).append(row)
        elif row.get("record_type") == "test":
            tests[path] = row
        else:
            raise ValueError(f"differential result row has an unknown type: {path}")
    result: dict[str, ItemOutcome] = {}
    for path, test in tests.items():
        rows_for_test = backends.get(path, [])
        resolved = test.get("resolved_status")
        duration = test.get("duration_s")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)):
            duration = None
        detail = next(
            (
                str(row["detail"])[:_DETAIL_CHARS]
                for row in rows_for_test
                if row.get("detail")
            ),
            "",
        )
        status: ItemStatus
        if resolved == "pass":
            status, reason = "passed", ""
        elif resolved == "skip":
            status, reason = "skipped", str(test.get("exclusion_reason") or "skip")
        elif resolved == "fail":
            status, reason = "failed", _failure_reason(test, rows_for_test)
        else:
            status, reason = "errors", _failure_reason(test, rows_for_test)
        result[path] = ItemOutcome(status, reason, detail, duration)
    return result
