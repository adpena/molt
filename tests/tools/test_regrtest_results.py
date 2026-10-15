from __future__ import annotations

from pathlib import Path

import pytest

from tools.compat import regrtest_results
from tools.compat.item_outcome import ITEM_STATUSES, ItemOutcome


def test_every_regrtest_state_has_one_shard_status() -> None:
    assert set(regrtest_results.STATUS_BY_STATE) == set(
        regrtest_results.REGRTEST_STATES
    )
    assert set(regrtest_results.STATUS_BY_STATE.values()) <= set(ITEM_STATUSES)


def test_rows_read_back_as_shard_outcomes(tmp_path: Path) -> None:
    log = tmp_path / "results.jsonl"
    regrtest_results.append(log, test="test_math", state="PASSED", duration_s=1.5)
    regrtest_results.append(
        log,
        test="test_os",
        state="TIMEOUT",
        duration_s=600.0,
        reason="timeout",
        detail="molt exceeded the 600 s test timeout",
    )
    regrtest_results.append(
        log, test="test_io", state="FAILED", duration_s=2.0, reason="molt rc=1"
    )
    regrtest_results.append(log, test="test_re", state="ENV_CHANGED", duration_s=3.0)
    regrtest_results.append(
        log,
        test="test_ssl",
        state="DID_NOT_RUN",
        duration_s=None,
        reason=regrtest_results.DEADLINE_REASON,
    )

    assert regrtest_results.outcomes(log) == {
        "test_math": ItemOutcome("passed", "", "", 1.5),
        "test_os": ItemOutcome(
            "errors", "timeout", "molt exceeded the 600 s test timeout", 600.0
        ),
        "test_io": ItemOutcome("failed", "molt rc=1", "", 2.0),
        # A row without a reason is named by its state.
        "test_re": ItemOutcome("failed", "env_changed", "", 3.0),
        "test_ssl": ItemOutcome("not_run", "deadline", "", None),
    }


def test_a_rerun_row_replaces_the_first(tmp_path: Path) -> None:
    log = tmp_path / "results.jsonl"
    regrtest_results.append(
        log, test="test_math", state="FAILED", duration_s=1.0, reason="molt rc=1"
    )
    regrtest_results.append(log, test="test_math", state="PASSED", duration_s=1.2)

    assert regrtest_results.outcomes(log)["test_math"].status == "passed"


@pytest.mark.parametrize(
    ("state", "duration_s"),
    [("DID_NOT_RUN", 1.0), ("PASSED", None), ("NOT_A_STATE", 1.0)],
)
def test_append_refuses_an_incoherent_row(
    tmp_path: Path, state: str, duration_s: float | None
) -> None:
    with pytest.raises(ValueError):
        regrtest_results.append(
            tmp_path / "results.jsonl",
            test="test_math",
            state=state,
            duration_s=duration_s,
        )


@pytest.mark.parametrize(
    "line",
    [
        '{"test": "test_math", "state": "PASSED"}',
        '{"test": "test_math", "state": "DID_NOT_RUN", "duration_s": 1.0}',
        '{"state": "PASSED", "duration_s": 1.0}',
        '{"test": "test_math", "state": "LOST", "duration_s": 1.0}',
        "not json",
    ],
)
def test_outcomes_refuse_a_malformed_row(tmp_path: Path, line: str) -> None:
    log = tmp_path / "results.jsonl"
    log.write_text(line + "\n", encoding="utf-8")

    with pytest.raises(ValueError):
        regrtest_results.outcomes(log)
