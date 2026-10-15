"""The one writer and reader of the regrtest per-test result log.

`tools/molt_regrtest_shim.py` runs each regrtest test in its own worker
process and appends one row as the test finishes or is refused admission, so
the log survives a killed run. `tools/cpython_regrtest.py` builds its
summary's item rows from the log, and the Nightly shard runner recovers the
finished tests from it after a kill.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path
from typing import Literal, get_args

from molt.exact_json import ExactJsonError, loads_exact
from tools.compat.item_outcome import ItemOutcome, ItemStatus

RESULTS_FILENAME = "item_results.jsonl"
DEADLINE_REASON = "deadline"
_DETAIL_CHARS = 160

# libregrtest's result states (Lib/test/libregrtest/result.py, `State`).
RegrtestState = Literal[
    "PASSED",
    "FAILED",
    "UNCAUGHT_EXC",
    "REFLEAK",
    "ENV_CHANGED",
    "SKIPPED",
    "RESOURCE_DENIED",
    "INTERRUPTED",
    "WORKER_FAILED",
    "WORKER_BUG",
    "TIMEOUT",
    "DID_NOT_RUN",
]
REGRTEST_STATES: tuple[RegrtestState, ...] = get_args(RegrtestState)

STATUS_BY_STATE: Mapping[RegrtestState, ItemStatus] = {
    "PASSED": "passed",
    "FAILED": "failed",
    "UNCAUGHT_EXC": "failed",
    "REFLEAK": "failed",
    "ENV_CHANGED": "failed",
    "SKIPPED": "skipped",
    "RESOURCE_DENIED": "skipped",
    "INTERRUPTED": "errors",
    "WORKER_FAILED": "errors",
    "WORKER_BUG": "errors",
    "TIMEOUT": "errors",
    "DID_NOT_RUN": "not_run",
}


def _state(value: object) -> RegrtestState:
    for state in REGRTEST_STATES:
        if value == state:
            return state
    raise ValueError(f"unknown regrtest state: {value!r}")


def append(
    path: Path,
    *,
    test: str,
    state: str,
    duration_s: float | None,
    reason: str = "",
    detail: str = "",
) -> None:
    """Append one test's row; a test that did not run has no duration."""
    checked = _state(state)
    if (duration_s is None) != (checked == "DID_NOT_RUN"):
        raise ValueError(f"regrtest row for {test} has an invalid duration")
    row = {
        "test": test,
        "state": checked,
        "duration_s": duration_s,
        "reason": reason,
        "detail": detail[:_DETAIL_CHARS],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    # Workers append concurrently; one short line is one write.
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row, sort_keys=True) + "\n")


def outcomes(path: Path) -> dict[str, ItemOutcome]:
    """Each test's outcome, keyed by module name; a later row replaces an earlier."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"regrtest result log is unreadable: {exc}") from exc
    result: dict[str, ItemOutcome] = {}
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = loads_exact(line)
        except (ValueError, ExactJsonError) as exc:
            raise ValueError(
                f"regrtest result line {line_number} is invalid: {exc}"
            ) from exc
        if not isinstance(row, Mapping) or not isinstance(row.get("test"), str):
            raise ValueError(f"regrtest result line {line_number} names no test")
        state = _state(row.get("state"))
        status = STATUS_BY_STATE[state]
        duration = row.get("duration_s")
        if status == "not_run":
            if duration is not None:
                raise ValueError(
                    f"regrtest result line {line_number} times an unrun test"
                )
        elif isinstance(duration, bool) or not isinstance(duration, (int, float)):
            raise ValueError(f"regrtest result line {line_number} has no duration")
        reason = str(row.get("reason") or "")
        if status != "passed" and not reason:
            reason = state.lower()
        result[row["test"]] = ItemOutcome(
            status=status,
            reason="" if status == "passed" else reason,
            detail=str(row.get("detail") or ""),
            duration_s=None if duration is None else float(duration),
        )
    return result
