"""Teeth for the CLAIMS terminal-status vocabulary + classifier (APPARATUS A11).

Proves the ask: terminal statuses parse + classify correctly (a FALSIFIED row is
RETIRED, not live), the self-retirement vocabulary retires a lane, staleness is
flagged (never auto-reclaimed), and the falsifiable ``--check`` self-test fails
when the classifier rots. A classifier that only ever says LIVE certifies nothing.
"""

from __future__ import annotations

import datetime as _dt
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import claims_status as cs  # noqa: E402

NOW = _dt.datetime(2026, 7, 11, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _ts(hours_ago: float) -> str:
    return (NOW - _dt.timedelta(hours=hours_ago)).strftime(cs._TS_FMT)


def _row(
    status: str, hours_ago: float = 0.5, lane: str = "L", note: str = ""
) -> cs.Row:
    return cs.Row(lane=lane, agent="a", utc=_ts(hours_ago), status=status, note=note)


# --- the terminal vocabulary exists and is coherent --------------------------


def test_self_retire_terminals_are_the_four_new_statuses() -> None:
    assert cs.SELF_RETIRE_TERMINALS == frozenset(
        {
            "FALSIFIED",
            "MEASURED_IMPLEMENTATION_RETIRED",
            "STALE_ASSUMED_DEAD",
            "SUPERSEDED",
        }
    )
    # They are terminal (retire the lane) but distinct from the positive terminals.
    assert cs.SELF_RETIRE_TERMINALS <= cs.TERMINAL_STATUSES
    assert not (cs.SELF_RETIRE_TERMINALS & cs.POSITIVE_TERMINALS)
    assert not (cs.TERMINAL_STATUSES & cs.LIVE_STATUSES)


# --- classify_status ---------------------------------------------------------


@pytest.mark.parametrize("status", sorted(cs.TERMINAL_STATUSES))
def test_every_terminal_status_classifies_retired(status: str) -> None:
    # Even a FRESH terminal row (0.1h old) is RETIRED, never live.
    klass, _ = cs.classify_status(_row(status, hours_ago=0.1), NOW)
    assert klass == cs.RETIRED, f"{status} must classify RETIRED"


def test_falsified_is_retired_not_live() -> None:
    klass, _ = cs.classify_status(_row("FALSIFIED", hours_ago=0.1), NOW)
    assert klass == cs.RETIRED


@pytest.mark.parametrize("status", sorted(cs.LIVE_STATUSES))
def test_fresh_live_status_is_live(status: str) -> None:
    klass, age = cs.classify_status(_row(status, hours_ago=0.5), NOW)
    assert klass == cs.LIVE
    assert age is not None and age < cs.STALE_HOURS


@pytest.mark.parametrize("status", sorted(cs.LIVE_STATUSES))
def test_old_live_status_is_stale(status: str) -> None:
    klass, age = cs.classify_status(_row(status, hours_ago=9.0), NOW)
    assert klass == cs.STALE
    assert age is not None and age > cs.STALE_HOURS


def test_unparseable_timestamp_is_live_not_stale() -> None:
    # We never assert stale on uncertainty (a preflight/report must not falsely
    # flag a live lane just because a timestamp is malformed).
    row = cs.Row(lane="L", agent="a", utc="not-a-date", status="CLAIMED", note="")
    klass, age = cs.classify_status(row, NOW)
    assert klass == cs.LIVE
    assert age is None


# --- self-retirement overrides an earlier live row ---------------------------


def test_later_falsified_retires_an_earlier_claimed_lane() -> None:
    rows = [
        _row("CLAIMED", hours_ago=99, lane="X"),
        _row("FALSIFIED", hours_ago=1, lane="X"),
    ]
    summ = cs.summarize(rows, NOW)
    assert [s.lane for s in summ.retired] == ["X"]
    assert not summ.live and not summ.stale


def test_superseded_frees_the_lane() -> None:
    rows = [
        _row("PROGRESS", hours_ago=2, lane="Y"),
        _row("SUPERSEDED", hours_ago=1, lane="Y"),
    ]
    summ = cs.summarize(rows, NOW)
    assert [s.lane for s in summ.retired] == ["Y"]


# --- parsing the real table format -------------------------------------------


def test_parse_rows_reads_the_log_table_and_skips_headers() -> None:
    text = (
        "# preamble\n"
        "| lane | agent-id | UTC | status | note |\n"  # before ## Log -> ignored
        "## Log\n"
        "| lane | agent-id | UTC (ISO) | status | note / evidence |\n"
        "|------|----------|-----------|--------|-----------------|\n"
        "| _(none yet)_ | | | | |\n"
        f"| A | ag | {_ts(1)} | CLAIMED | started |\n"
        f"| A | ag | {_ts(0.5)} | FALSIFIED | premise disproven: no such symbol |\n"
        f"| B | bg | {_ts(0.5)} | COMPLETE | landed deadbeef | with | pipes |\n"
    )
    rows = cs.parse_rows(text)
    lanes = [r.lane for r in rows]
    assert lanes == ["A", "A", "B"]
    # a note containing pipes is rejoined, not truncated
    b = [r for r in rows if r.lane == "B"][0]
    assert b.note == "landed deadbeef | with | pipes"
    # latest-per-lane picks the FALSIFIED row for A
    latest = cs.latest_by_lane(rows)
    assert latest["A"].status == "FALSIFIED"


def test_summarize_counts_and_buckets() -> None:
    rows = [
        _row("CLAIMED", 0.5, lane="live1"),
        _row("PROGRESS", 30.0, lane="stale1"),
        _row("COMPLETE", 100.0, lane="done1"),
        _row("FALSIFIED", 2.0, lane="dead1"),
    ]
    summ = cs.summarize(rows, NOW)
    d = summ.as_dict()
    assert d["counts"] == {"live": 1, "stale": 1, "retired": 2}


# --- the falsifiable self-test has teeth -------------------------------------


def test_selftest_passes_clean() -> None:
    code, failures = cs._run_selftest()
    assert code == 0 and failures == []


def test_selftest_fails_if_terminal_misclassified(monkeypatch) -> None:
    # Simulate the vocabulary rotting: FALSIFIED silently dropped from terminals.
    monkeypatch.setattr(cs, "TERMINAL_STATUSES", cs.POSITIVE_TERMINALS)
    code, failures = cs._run_selftest()
    assert code == 1
    assert any("falsified" in f.lower() for f in failures)


def test_cli_check_exit_code() -> None:
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "claims_status.py"), "--check"],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr
    assert "pass" in proc.stdout.lower()


def test_cli_live_report_is_warn_only(tmp_path: Path) -> None:
    # Even with a stale CLAIMED lane, the live report exits 0 (flagged, not failed).
    claims = tmp_path / "CLAIMS.md"
    claims.write_text(
        "## Log\n"
        "| lane | agent-id | UTC (ISO) | status | note |\n"
        "|------|----------|-----------|--------|------|\n"
        f"| STALE-LANE | ag | {_ts(99)} | CLAIMED | silent |\n",
        encoding="utf-8",
    )
    proc = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools" / "claims_status.py"),
            "--path",
            str(claims),
        ],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == 0
    assert "1 stale" in proc.stdout
    assert "STALE-LANE" in proc.stdout
