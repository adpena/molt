"""Teeth for the premise-verification preflight (APPARATUS A11).

Proves the ask: exit 8 STAND_DOWN_DUPLICATE on a synthetic already-landed CLAIMS
row for the same files, exit 9 WAIT_AND_REASSESS on a mid-flight CLAIMED row for
the same files, exit 0 otherwise; landed outranks inflight; a recent git commit
touching a target file is a landed signal; and the preflight fails OPEN (proceed)
when the ledger cannot be read. A preflight that always says PROCEED certifies
nothing.
"""

from __future__ import annotations

import datetime as _dt
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import check_sister_landed as csl  # noqa: E402
from tools import claims_status as cs  # noqa: E402

NOW = _dt.datetime(2026, 7, 11, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _ts(hours_ago: float) -> str:
    return (NOW - _dt.timedelta(hours=hours_ago)).strftime(cs._TS_FMT)


def _claims(*rows: str) -> str:
    head = (
        "## Log\n"
        "| lane | agent-id | UTC (ISO) | status | note / evidence |\n"
        "|------|----------|-----------|--------|-----------------|\n"
    )
    return head + "".join(rows)


def _row(lane: str, status: str, note: str, hours_ago: float = 1.0) -> str:
    return f"| {lane} | ag | {_ts(hours_ago)} | {status} | {note} |\n"


# --- pure classifier ---------------------------------------------------------


def test_classify_landed_stands_down() -> None:
    v = csl.classify(["LANE (COMPLETE)"], [])
    assert v.code == csl.STAND_DOWN_DUPLICATE == 8


def test_classify_inflight_waits() -> None:
    v = csl.classify([], ["LANE (CLAIMED)"])
    assert v.code == csl.WAIT_AND_REASSESS == 9


def test_classify_clean_proceeds() -> None:
    v = csl.classify([], [])
    assert v.code == csl.PROCEED == 0


def test_landed_outranks_inflight() -> None:
    v = csl.classify(["done"], ["mid"])
    assert v.code == csl.STAND_DOWN_DUPLICATE


# --- matching ----------------------------------------------------------------


def test_row_matches_by_file_path_and_basename() -> None:
    assert csl.row_matches_target(
        "landed runtime/foo/bar.rs", "L", ["runtime/foo/bar.rs"], None
    )
    assert csl.row_matches_target("touched bar.rs", "L", ["runtime/foo/bar.rs"], None)
    assert not csl.row_matches_target(
        "unrelated note", "L", ["runtime/foo/bar.rs"], None
    )


def test_row_matches_by_topic_in_lane_or_note() -> None:
    assert csl.row_matches_target("note", "E1-WITNESS-TO-GREEN", [], "witness")
    assert csl.row_matches_target("about the witness seal", "OTHER", [], "witness")
    assert not csl.row_matches_target("note", "OTHER", [], "witness")


# --- claims_matches splits landed vs inflight by terminal-vs-not --------------


def test_already_landed_claim_is_a_landed_match() -> None:
    text = _claims(_row("DONE", "COMPLETE", "landed abc in src/foo.rs", hours_ago=100))
    landed, inflight = csl.claims_matches(text, ["src/foo.rs"], None, NOW)
    assert landed and not inflight


def test_mid_flight_claim_is_an_inflight_match() -> None:
    text = _claims(_row("LIVE", "CLAIMED", "working src/bar.py", hours_ago=1))
    landed, inflight = csl.claims_matches(text, ["src/bar.py"], None, NOW)
    assert inflight and not landed


def test_stale_claim_is_still_inflight_not_ignored() -> None:
    # A stale claim may be a silently-working sister -> WAIT, never PROCEED (CLAIMS.md 6).
    text = _claims(_row("SILENT", "CLAIMED", "working src/baz.py", hours_ago=99))
    landed, inflight = csl.claims_matches(text, ["src/baz.py"], None, NOW)
    assert inflight and not landed
    assert csl.classify(landed, inflight).code == csl.WAIT_AND_REASSESS


def test_falsified_sister_does_not_block_a_fresh_lane() -> None:
    # A FALSIFIED sister LANE on the same files is RETIRED -> it is a "landed"
    # (resolved) signal, which correctly stands a duplicate down rather than
    # letting it silently re-run the disproven premise.
    text = _claims(
        _row("DEAD", "FALSIFIED", "premise disproven for src/q.rs", hours_ago=1)
    )
    landed, inflight = csl.claims_matches(text, ["src/q.rs"], None, NOW)
    assert landed and not inflight


# --- git recent-file signal (real temp repo) ---------------------------------


def _git(root: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args], cwd=str(root), capture_output=True, text=True, encoding="utf-8"
    )


def _init_repo(root: Path) -> None:
    _git(root, "init", "-q")
    _git(root, "config", "user.email", "a@b.c")
    _git(root, "config", "user.name", "t")
    _git(root, "config", "commit.gpgsign", "false")


def test_git_recent_commit_is_a_landed_signal(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    (tmp_path / "surface.rs").write_text("fn x() {}\n", encoding="utf-8")
    _git(tmp_path, "add", "surface.rs")
    _git(tmp_path, "commit", "-q", "-m", "land surface")
    touched = csl.git_recent_file_matches(tmp_path, ["surface.rs"], hours=24.0)
    assert "surface.rs" in touched
    # a brand-new (uncommitted) file is NOT a false match
    assert csl.git_recent_file_matches(tmp_path, ["never.rs"], hours=24.0) == []


def test_evaluate_stands_down_on_git_landed_surface(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    (tmp_path / "landed.py").write_text("x = 1\n", encoding="utf-8")
    _git(tmp_path, "add", "landed.py")
    _git(tmp_path, "commit", "-q", "-m", "land it")
    # no CLAIMS file -> only the git signal fires
    v = csl.evaluate(
        tmp_path, ["landed.py"], None, hours=24.0, claims_path=tmp_path / "nope.md"
    )
    assert v.code == csl.STAND_DOWN_DUPLICATE


def test_evaluate_fails_open_when_ledger_unreadable(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    v = csl.evaluate(
        tmp_path,
        ["brand/new/file.rs"],
        None,
        hours=24.0,
        claims_path=tmp_path / "missing.md",
    )
    assert v.code == csl.PROCEED


# --- the falsifiable self-test + CLI -----------------------------------------


def test_selftest_passes_clean() -> None:
    code, failures = csl._run_selftest()
    assert code == 0 and failures == []


def test_selftest_fails_if_precedence_rots(monkeypatch) -> None:
    # Simulate classify losing its landed>inflight precedence (always proceeds).
    monkeypatch.setattr(
        csl, "classify", lambda landed, inflight: csl.Verdict(0, "PROCEED", "")
    )
    code, failures = csl._run_selftest()
    assert code == 1 and failures


def test_cli_check_exit_code() -> None:
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "check_sister_landed.py"), "--check"],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr


def test_cli_usage_error_without_target() -> None:
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "check_sister_landed.py")],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == csl.USAGE_ERROR


def test_cli_stands_down_on_matching_claims(tmp_path: Path) -> None:
    claims = tmp_path / "CLAIMS.md"
    claims.write_text(
        _claims(_row("DONE", "COMPLETE", "landed in src/done.rs", 100)),
        encoding="utf-8",
    )
    proc = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools" / "check_sister_landed.py"),
            "--files",
            "src/done.rs",
            "--claims",
            str(claims),
            "--root",
            str(tmp_path),
        ],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == csl.STAND_DOWN_DUPLICATE
    assert "STAND_DOWN_DUPLICATE" in proc.stdout
