"""Teeth for tools/claim_lane.py: the solo-lane claim protocol must be atomic
and fail-closed (back off on a live claim; allow claim on free/stale/released)."""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import datetime as dt
import subprocess
import sys
from pathlib import Path

import pytest

TOOLS = Path(__file__).resolve().parents[2] / "tools"
CLAIM = TOOLS / "claim_lane.py"


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def _run(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(CLAIM), *args], cwd=repo, capture_output=True, text=True
    )


def _claims_body(rows: str) -> str:
    return (
        "# Solo-Owner Lane Claims\n\n"
        "## Log (append-only)\n\n"
        "| lane | agent-id | UTC (ISO) | status | note / evidence |\n"
        "|------|----------|-----------|--------|-----------------|\n" + rows
    )


@pytest.fixture()
def repo(tmp_path: Path) -> Path:
    bare = tmp_path / "remote.git"
    bare.mkdir()
    _git(bare, "init", "--bare", "-q", "-b", "main")
    w = tmp_path / "work"
    (w / "docs" / "agent").mkdir(parents=True)
    _git(w, "init", "-q", "-b", "main")
    _git(w, "config", "user.email", "t@t")
    _git(w, "config", "user.name", "t")
    _git(w, "remote", "add", "origin", str(bare))
    (w / "docs" / "agent" / "CLAIMS.md").write_text(_claims_body(""), encoding="utf-8")
    _git(w, "add", "-A")
    _git(w, "commit", "-q", "-m", "init")
    _git(w, "push", "-q", "origin", "main")
    return w


def _set_claims(repo: Path, rows: str) -> None:
    (repo / "docs" / "agent" / "CLAIMS.md").write_text(
        _claims_body(rows), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-m", "seed claims", "--", "docs/agent/CLAIMS.md")
    _git(repo, "push", "-q", "origin", "main")


def _row(lane: str, agent: str, when: dt.datetime, status: str, note: str = "x") -> str:
    return f"| {lane} | {agent} | {when.strftime('%Y-%m-%dT%H:%M:%SZ')} | {status} | {note} |\n"


def test_check_unclaimed_is_claimable(repo: Path) -> None:
    res = _run(repo, "L1", "--check")
    assert res.returncode == 0 and "UNCLAIMED" in res.stdout


def test_check_live_claim_backs_off(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "other", now, "CLAIMED"))
    res = _run(repo, "L1", "--check")
    assert res.returncode == 1 and "CLAIMED-ALIVE" in res.stdout


def test_check_stale_claim_is_reclaimable(repo: Path) -> None:
    old = dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=5)
    _set_claims(repo, _row("L1", "other", old, "CLAIMED"))
    res = _run(repo, "L1", "--check")
    assert res.returncode == 0 and "STALE" in res.stdout


def test_claim_free_lane_lands(repo: Path) -> None:
    res = _run(repo, "L1", "--claim", "--agent", "me", "--note", "go")
    assert res.returncode == 0, res.stdout
    assert "CLAIMED L1 as me" in res.stdout
    _git(repo, "fetch", "origin", "--quiet")
    assert "| L1 | me |" in _git(repo, "show", "origin/main:docs/agent/CLAIMS.md")


def test_claim_held_lane_refused(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "other", now, "CLAIMED"))
    before = _git(repo, "rev-parse", "origin/main")
    res = _run(repo, "L1", "--claim", "--agent", "me")
    assert res.returncode == 1 and "BACK OFF" in res.stdout
    _git(repo, "fetch", "origin", "--quiet")
    assert _git(repo, "rev-parse", "origin/main") == before  # nothing landed


def test_append_progress_by_claimant_lands(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "me", now, "CLAIMED"))
    res = _run(repo, "L1", "--append", "PROGRESS", "--agent", "me", "--note", "run-123")
    assert res.returncode == 0, res.stdout
    assert "PROGRESS" in _git(repo, "show", "origin/main:docs/agent/CLAIMS.md")


def test_live_takeover_by_other_refused(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "owner", now, "CLAIMED"))
    res = _run(repo, "L1", "--append", "PROGRESS", "--agent", "intruder")
    assert res.returncode == 1 and "REFUSED" in res.stdout


def test_check_refuses_a_log_with_an_unknown_status(repo: Path) -> None:
    # The old reader skipped the newest row and reported the older CLAIMED one.
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(
        repo,
        _row("L1", "other", now - dt.timedelta(hours=1), "CLAIMED")
        + _row("L1", "other", now, "BLOCKED"),
    )
    res = _run(repo, "L1", "--check")
    assert res.returncode == 2, res.stdout
    assert "'BLOCKED'" in res.stderr


def test_append_refuses_a_note_that_breaks_the_table(repo: Path) -> None:
    before = _git(repo, "rev-parse", "origin/main")
    res = _run(repo, "L1", "--claim", "--agent", "me", "--note", "a | b")
    assert res.returncode == 2 and "--note" in res.stdout
    _git(repo, "fetch", "origin", "--quiet")
    assert _git(repo, "rev-parse", "origin/main") == before
